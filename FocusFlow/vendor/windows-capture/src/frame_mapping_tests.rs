// FocusFlow native regression: requires Windows Graphics Tools (D3D11 debug layer).
// These exercise the exact shared mapping constructor used by buffer and buffer_crop.
use super::*;
use windows::core::Interface;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;

fn device() -> (ID3D11Device, ID3D11DeviceContext, ID3D11InfoQueue) {
    let (mut device, mut context) = (None, None);
    unsafe {
        D3D11CreateDevice(None, D3D_DRIVER_TYPE_WARP, HMODULE::default(),
            D3D11_CREATE_DEVICE_DEBUG | D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&[D3D_FEATURE_LEVEL_11_0]), D3D11_SDK_VERSION,
            Some(&mut device), None, Some(&mut context))
            .expect("Native mapping gate requires WARP and the D3D11 debug layer; install Windows Graphics Tools");
    }
    let device = device.unwrap();
    let queue = device.cast::<ID3D11InfoQueue>().unwrap();
    (device, context.unwrap(), queue)
}

fn texture(device: &ID3D11Device, staging: bool) -> ID3D11Texture2D {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: 3, Height: 2, MipLevels: 1, ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: if staging { D3D11_USAGE_STAGING } else { D3D11_USAGE_DEFAULT },
        CPUAccessFlags: if staging { (D3D11_CPU_ACCESS_READ.0 | D3D11_CPU_ACCESS_WRITE.0) as u32 } else { 0 },
        ..Default::default()
    };
    let bytes: [u8; 24] = std::array::from_fn(|n| n as u8 + 1);
    let data = D3D11_SUBRESOURCE_DATA { pSysMem: bytes.as_ptr().cast(), SysMemPitch: 12, SysMemSlicePitch: 24 };
    let mut texture = None;
    unsafe { device.CreateTexture2D(&desc, Some(&data), Some(&mut texture)).unwrap(); }
    texture.unwrap()
}

fn messages(queue: &ID3D11InfoQueue) -> Vec<(D3D11_MESSAGE_SEVERITY, String)> {
    let mut result = Vec::new();
    unsafe {
        for index in 0..queue.GetNumStoredMessages() {
            let mut size = 0;
            queue.GetMessage(index, None, &mut size).unwrap();
            // usize allocation supplies pointer alignment required by D3D11_MESSAGE.
            let mut storage = vec![0usize; size.div_ceil(std::mem::size_of::<usize>())];
            let message = storage.as_mut_ptr().cast::<D3D11_MESSAGE>();
            queue.GetMessage(index, Some(message), &mut size).unwrap();
            let message = &*message;
            let description = slice::from_raw_parts(message.pDescription, message.DescriptionByteLength);
            result.push((message.Severity, String::from_utf8_lossy(description).into_owned()));
        }
    }
    result
}

fn assert_clean(queue: &ID3D11InfoQueue) {
    let failures: Vec<_> = messages(queue).into_iter().filter(|(severity, _)|
        *severity == D3D11_MESSAGE_SEVERITY_CORRUPTION || *severity == D3D11_MESSAGE_SEVERITY_ERROR ||
        *severity == D3D11_MESSAGE_SEVERITY_WARNING).collect();
    assert!(failures.is_empty(), "D3D11 debug diagnostics: {failures:?}");
}

fn remap(context: &ID3D11DeviceContext, texture: &ID3D11Texture2D) {
    // Actual API remap detects a missing Unmap, rather than a simulated guard counter.
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    unsafe {
        context.Map(texture, 0, D3D11_MAP_READ_WRITE, 0, Some(&mut mapped)).unwrap();
        context.Unmap(texture, 0);
    }
}

#[test]
fn mapped_owner_retains_resource_and_preserves_copied_rows() {
    let (device, context, queue) = device();
    let source = texture(&device, false);
    let staging = texture(&device, true);
    unsafe { context.CopyResource(&staging, &source); }
    drop(source);
    let mut buffer = FrameBuffer::from_texture(&context, staging, ColorFormat::Bgra8).unwrap();
    // No external staging owner survives the move. The buffer owns the mapped resource/context.
    let owner = buffer._mapping.as_ref().expect("mapped buffer must retain its owner");
    assert_eq!(owner.context.as_raw(), context.as_raw());
    let retained = owner.texture.clone();
    let pitch = buffer.row_pitch() as usize;
    let mut owned = Vec::new();
    for row in 0..2 { owned.extend_from_slice(&buffer.as_raw_buffer()[row * pitch..row * pitch + 12]); }
    drop(buffer);
    assert_eq!(owned, (1u8..=24).collect::<Vec<_>>());
    remap(&context, &retained);
    assert_clean(&queue);
}

#[test]
fn mapped_owner_unmaps_on_post_map_validation_error() {
    let (device, context, queue) = device();
    let staging = texture(&device, true);
    // Wrong format is rejected after successful Map; the installed guard must clean it up.
    assert!(FrameBuffer::from_texture(&context, staging.clone(), ColorFormat::Rgba16F).is_err());
    remap(&context, &staging);
    assert_clean(&queue);
}

#[test]
fn mapped_owner_unmaps_on_copy_error_and_unwind() {
    let (device, context, queue) = device();
    let staging = texture(&device, true);
    let result = (|| -> Result<(), &'static str> {
        let _buffer = FrameBuffer::from_texture(&context, staging.clone(), ColorFormat::Bgra8).unwrap();
        Err("injected owned-copy allocation failure")
    })();
    assert!(result.is_err());
    remap(&context, &staging);
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _buffer = FrameBuffer::from_texture(&context, staging.clone(), ColorFormat::Bgra8).unwrap();
        panic!("injected owned-copy unwind");
    }));
    assert!(panic.is_err());
    remap(&context, &staging);
    assert_clean(&queue);
}

#[test]
fn failed_map_does_not_unmap() {
    let (device, context, queue) = device();
    unsafe { queue.ClearStoredMessages(); }
    assert!(FrameBuffer::from_texture(&context, texture(&device, false), ColorFormat::Bgra8).is_err());
    let diagnostics = messages(&queue);
    assert!(!diagnostics.is_empty(), "debug layer must report the deliberately invalid Map");
    for (_, description) in diagnostics {
        let text = description.to_ascii_lowercase();
        assert!(text.contains("map") && !text.contains("unmap"), "Unexpected diagnostic: {description}");
    }
}
