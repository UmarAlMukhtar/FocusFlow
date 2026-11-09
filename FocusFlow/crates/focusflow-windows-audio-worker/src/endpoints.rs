//! Bounded read-only output-device inventory; selecting an endpoint does not enable capture.
use crate::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind { System, Microphone }

#[derive(Debug, Clone)]
pub struct Endpoint {
    pub kind: Kind,
    pub id: String,
    pub display_name: String,
}

pub(crate) const MAX_ENDPOINTS: u32 = 32;
pub(crate) const MAX_ID_BYTES: usize = 2048;
pub(crate) const MAX_LABEL_BYTES: usize = 512;

pub(crate) fn decode_text(units: impl Iterator<Item = u16>, cap: usize) -> Result<String> {
    let mut collected = Vec::new();
    for unit in units.take(cap + 1) {
        if unit == 0 {
            let text = String::from_utf16(&collected).map_err(|_| "An output device returned malformed text")?;
            return if text.len() <= cap { Ok(text) } else { Err("Output device text exceeds the supported length".into()) };
        }
        collected.push(unit);
    }
    Err("Output device text is unterminated or exceeds the supported length".into())
}

pub(crate) fn collect_outputs(count: u32, mut get: impl FnMut(u32) -> Result<Endpoint>) -> Result<Vec<Endpoint>> {
    if count > MAX_ENDPOINTS { return Err("Too many output devices; at most 32 are supported".into()); }
    let mut result: Vec<Endpoint> = Vec::with_capacity(count as usize);
    for index in 0..count {
        let endpoint = get(index)?;
        if endpoint.kind != Kind::System { return Err("An output device changed type; refresh the device list".into()); }
        for (text, limit) in [(&endpoint.id, MAX_ID_BYTES), (&endpoint.display_name, MAX_LABEL_BYTES)] {
            if text.trim().is_empty() || text.len() > limit || text.chars().any(char::is_control) {
                return Err("An output device returned an invalid identifier or display name".into());
            }
        }
        if result.iter().any(|previous| previous.id == endpoint.id) {
            return Err("Output devices returned duplicate identifiers; refresh the device list".into());
        }
        result.push(endpoint);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn endpoint(id: &str, label: &str) -> Endpoint {
        Endpoint { kind: Kind::System, id: id.into(), display_name: label.into() }
    }
    #[test]
    fn empty_inventory_does_not_query_an_endpoint() {
        assert!(collect_outputs(0, |_| panic!("empty inventory")).unwrap().is_empty());
    }
    #[test]
    fn valid_inventory_preserves_exact_ids_and_duplicate_friendly_labels() {
        let devices = collect_outputs(2, |i| Ok(endpoint(if i == 0 { "Opaque-A" } else { "opaque-b" }, "Speakers"))).unwrap();
        assert_eq!(devices[0].id, "Opaque-A");
        assert_eq!(devices[1].id, "opaque-b");
        assert_eq!(devices[0].display_name, devices[1].display_name);
    }
    #[test]
    fn excess_count_is_rejected_before_any_endpoint_query() {
        assert!(collect_outputs(33, |_| panic!("must cap before query")).is_err());
    }
    #[test]
    fn maximum_inventory_is_accepted_without_truncation() {
        assert_eq!(collect_outputs(32, |i| Ok(endpoint(&i.to_string(), "Speakers"))).unwrap().len(), 32);
    }
    #[test]
    fn duplicate_ids_are_rejected_instead_of_presented_as_distinct_choices() {
        assert!(collect_outputs(2, |i| Ok(endpoint("same", if i == 0 { "A" } else { "B" }))).is_err());
    }
    #[test]
    fn invalid_identity_label_or_kind_fails_the_inventory() {
        for (id, label) in [("", "A"), ("  ", "A"), ("id\0", "A"), ("id\n", "A"), ("id", ""), ("id", "\t"), ("id", "bad\nlabel")] {
            assert!(collect_outputs(1, |_| Ok(endpoint(id, label))).is_err(), "{id:?} {label:?}");
        }
        assert!(collect_outputs(1, |_| Ok(Endpoint { kind: Kind::Microphone, id: "id".into(), display_name: "Mic".into() })).is_err());
    }
    #[test]
    fn text_byte_limits_apply_without_truncation() {
        assert!(collect_outputs(1, |_| Ok(endpoint(&"x".repeat(2048), &"é".repeat(256)))).is_ok());
        assert!(collect_outputs(1, |_| Ok(endpoint(&"x".repeat(2049), "A"))).is_err());
        assert!(collect_outputs(1, |_| Ok(endpoint("id", &"é".repeat(257)))).is_err());
    }
    #[test]
    fn query_failure_does_not_return_a_partial_device_list() {
        assert!(collect_outputs(2, |i| if i == 0 { Ok(endpoint("a", "A")) } else { Err("disconnected".into()) }).is_err());
    }
    #[test]
    fn utf16_requires_terminator_valid_encoding_and_bounded_utf8() {
        assert_eq!(decode_text([65, 0, 0xd800].into_iter(), 1).unwrap(), "A");
        assert!(decode_text([0xd800, 0].into_iter(), 4).is_err());
        assert!(decode_text([65].into_iter(), 1).is_err());
        assert!(decode_text([0x00e9, 0].into_iter(), 1).is_err());
    }
    #[test]
    fn utf16_limit_bounds_reads_even_without_terminator() {
        let mut reads = 0;
        let units = std::iter::from_fn(|| { reads += 1; assert!(reads <= 5); Some(65) });
        assert!(decode_text(units, 4).is_err());
        assert_eq!(reads, 5);
    }
}
