# FocusFlow local patch

Upstream: windows-capture 2.0.0, published crates.io archive SHA-256 0f9460c7b82f6b7d314d85b45610481f26a1159a42dadf1e13a329041553e060. The archive was verified against the original app lockfile before extraction. All published files are retained unchanged except src/frame.rs. Upstream LICENCE applies; dependency versions and registry cache files are unchanged. No registry checksum metadata is claimed for this patched copy.

Local changes:

- A safe public content_size accessor exposes Direct3D11CaptureFrame.ContentSize, detecting old-texture/new-content resize before pixel admission.
- Both buffer and buffer_crop retain the mapped staging texture and its device context in an internal FrameBuffer owner. Successful Map installs the owner before validation or CPU reads; its Drop calls Unmap before releasing either COM owner, including error/unwind paths. Failed Map installs no owner. Public constructors/accessors retain their signatures and returned views borrow the buffer.
- src/frame_mapping_tests.rs adds native WARP/debug-layer regressions for resource retention, copied row bytes, cleanup/remap after normal/error/unwind exits, and no Unmap on failed Map. These require native Windows execution; metadata compilation alone does not qualify the runtime path.

Provenance and exact diffs are retained under .superpowers/sdd/2026-10-27-windows-owned-product: vendor-upstream.json records published hashes; vendor-accessor.patch is the historical initial accessor-only patch; fix-1-vendor-cumulative.patch includes the accessor and mapping correction/tests; fix-1-vendor-provenance.json records original and corrected file hashes. The original Task 1 evidence remains unchanged.
