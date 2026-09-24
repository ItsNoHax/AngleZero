//! Cars packed into the EBOOT.
//!
//! The console reads this index off the memory stick with nothing to fall back on if it is wrong,
//! so everything it trusts about it — names, where each car is, that nothing points past the end —
//! is checked here, against bundles built with the same encoders the asset tool writes them with.

use angle_zero::bundle::{
    encode_entry, encode_header, index_bytes, psar_offset, Error, Index, Span, ALIGN, ENTRY_BYTES,
    HEADER_BYTES, PBP_HEADER_BYTES,
};
use angle_zero::catalogue::{MAX_ENTRIES, NAME_MAX};
use std::vec::Vec;

/// Packs `cars` the way the asset tool does: index, then each car on an `ALIGN` boundary.
fn pack(cars: &[(&[u8], &[u8])]) -> Vec<u8> {
    let mut at = index_bytes(cars.len());
    let mut spans = Vec::new();
    for (_, bytes) in cars {
        at = at.next_multiple_of(ALIGN);
        spans.push(Span { offset: at, size: bytes.len() });
        at += bytes.len();
    }
    let mut out = encode_header(cars.len()).to_vec();
    for ((name, _), span) in cars.iter().zip(&spans) {
        out.extend_from_slice(&encode_entry(name, *span).unwrap());
    }
    for ((_, bytes), span) in cars.iter().zip(&spans) {
        out.resize(span.offset, 0);
        out.extend_from_slice(bytes);
    }
    out
}

#[test]
fn every_car_packed_is_found_again_byte_for_byte() {
    let cars: [(&[u8], &[u8]); 3] = [
        (b"bmw_e36.azcar", b"first car"),
        (b"nissan_s15.azcar", b"a second, longer car"),
        (b"toyota_ae86.azcar", b"3"),
    ];
    let bundle = pack(&cars);
    let index = Index::parse(&bundle, bundle.len()).unwrap();
    assert_eq!(index.len(), 3);
    for (i, (name, bytes)) in cars.iter().enumerate() {
        assert_eq!(index.name(i), *name);
        let span = index.find(name).unwrap();
        assert_eq!(span.offset % ALIGN, 0);
        assert_eq!(&bundle[span.offset..span.offset + span.size], *bytes);
    }
    assert!(index.find(b"missing.azcar").is_none());
}

#[test]
fn the_index_alone_is_enough_to_list_the_cars() {
    // The console reads the index and nothing else until a car is picked.
    let bundle = pack(&[(b"a.azcar", &[7; 5000]), (b"b.azcar", &[9; 5000])]);
    let index = Index::parse(&bundle[..index_bytes(2)], bundle.len()).unwrap();
    assert_eq!(index.name(1), b"b.azcar");
}

#[test]
fn a_name_that_fills_the_field_has_no_terminator_and_still_reads_back() {
    let mut name = [b'x'; NAME_MAX];
    name[NAME_MAX - 6..].copy_from_slice(b".azcar");
    let bundle = pack(&[(&name, b"car")]);
    let index = Index::parse(&bundle, bundle.len()).unwrap();
    assert_eq!(index.name(0), name);
}

#[test]
fn anything_that_is_not_a_bundle_says_so_rather_than_being_damaged() {
    // A build straight out of `cargo psp` has an empty PSAR; some other tool's PSAR is not ours.
    assert_eq!(Index::parse(&[], 0).err(), Some(Error::Absent));
    assert_eq!(Index::parse(&[0u8; 64], 64).err(), Some(Error::Absent));
}

#[test]
fn a_bundle_from_another_version_is_refused() {
    let mut bundle = pack(&[(b"a.azcar", b"car")]);
    bundle[4] = 2;
    assert_eq!(Index::parse(&bundle, bundle.len()).err(), Some(Error::Version));
}

#[test]
fn a_bundle_cut_short_is_refused_before_any_car_is_read() {
    let bundle = pack(&[(b"a.azcar", b"a car of some length")]);
    // The file was truncated inside the last car.
    assert_eq!(Index::parse(&bundle, bundle.len() - 1).err(), Some(Error::Truncated));
    // Or inside the index itself.
    assert_eq!(Index::parse(&bundle[..HEADER_BYTES + 3], bundle.len()).err(), Some(Error::Truncated));
}

#[test]
fn an_entry_pointing_into_the_index_is_refused() {
    let mut bundle = pack(&[(b"a.azcar", b"car")]);
    let at = HEADER_BYTES + NAME_MAX;
    bundle[at..at + 4].copy_from_slice(&0u32.to_le_bytes());
    assert_eq!(Index::parse(&bundle, bundle.len()).err(), Some(Error::BadEntry));
}

#[test]
fn more_cars_than_the_list_holds_is_refused() {
    let mut bundle = encode_header(MAX_ENTRIES + 1).to_vec();
    bundle.resize(index_bytes(MAX_ENTRIES + 1), 0);
    assert_eq!(Index::parse(&bundle, bundle.len()).err(), Some(Error::TooMany));
}

#[test]
fn only_names_a_catalogue_would_list_can_be_written() {
    let span = Span { offset: 64, size: 1 };
    assert!(encode_entry(b"car.azcar", span).is_ok());
    assert_eq!(encode_entry(b"readme.txt", span).err(), Some(Error::BadEntry));
    assert_eq!(encode_entry(b".azcar", span).err(), Some(Error::BadEntry));
    assert_eq!(encode_entry(&[b'x'; NAME_MAX + 1], span).err(), Some(Error::BadEntry));
    assert_eq!(ENTRY_BYTES, NAME_MAX + 8);
}

#[test]
fn the_psar_offset_is_the_last_of_the_eight_pbp_offsets() {
    let mut header = [0u8; PBP_HEADER_BYTES];
    header[..4].copy_from_slice(b"\0PBP");
    header[36..40].copy_from_slice(&988_780u32.to_le_bytes());
    assert_eq!(psar_offset(&header), Ok(988_780));
    header[1] = b'X';
    assert_eq!(psar_offset(&header), Err(Error::NotAPbp));
    assert_eq!(psar_offset(&header[..20]), Err(Error::NotAPbp));
}
