//! The list of cars on the stick.
//!
//! The console's directory walk hands entries over in whatever order FAT stored them, which is the
//! order somebody copied files in. Everything a player sees about that list — its order, what each
//! car is called before its file has been read, and what happens past the end of the table — is
//! decided here, where it can be tested without a PSP.

use angle_zero::catalogue::{Catalogue, Error, is_car_file, MAX_ENTRIES, NAME_MAX};

fn names(c: &Catalogue) -> std::vec::Vec<std::string::String> {
    (0..c.len())
        .map(|i| std::string::String::from_utf8(c.get(i).unwrap().name().to_vec()).unwrap())
        .collect()
}

#[test]
fn a_fresh_catalogue_holds_nothing() {
    let c = Catalogue::EMPTY;
    assert!(c.is_empty());
    assert_eq!(c.len(), 0);
    assert!(c.get(0).is_none());
}

#[test]
fn cars_are_listed_in_name_order_whatever_order_they_arrive_in() {
    let mut c = Catalogue::EMPTY;
    for name in [
        b"vw_golf_r.azcar".as_slice(),
        b"bmw_e36.azcar",
        b"toyota_ae86.azcar",
        b"bmw_e39.azcar",
    ] {
        c.insert(name).unwrap();
    }
    assert_eq!(
        names(&c),
        [
            "bmw_e36.azcar",
            "bmw_e39.azcar",
            "toyota_ae86.azcar",
            "vw_golf_r.azcar"
        ]
    );
}

#[test]
fn case_does_not_split_the_ordering() {
    // FAT hands back whatever case the copying machine wrote, so two spellings of the same fleet
    // have to interleave by name rather than by case.
    let mut c = Catalogue::EMPTY;
    for name in [
        b"NISSAN_S15.AZCAR".as_slice(),
        b"bmw_e36.azcar",
        b"Nissan_R34.azcar",
    ] {
        c.insert(name).unwrap();
    }
    assert_eq!(
        names(&c),
        ["bmw_e36.azcar", "Nissan_R34.azcar", "NISSAN_S15.AZCAR"]
    );
}

#[test]
fn a_prefix_sorts_before_what_extends_it() {
    let mut c = Catalogue::EMPTY;
    c.insert(b"bmw_e36_touring.azcar").unwrap();
    c.insert(b"bmw_e36.azcar").unwrap();
    assert_eq!(names(&c), ["bmw_e36.azcar", "bmw_e36_touring.azcar"]);
}

#[test]
fn a_name_is_kept_whole_and_not_padded() {
    // The table is fixed-size and every entry is a 64-byte buffer, so a name has to come back as
    // what was filed rather than as what it was stored in. A path built from a padded name is a
    // path to a file that does not exist.
    let mut c = Catalogue::EMPTY;
    c.insert(b"toyota_ae86.azcar").unwrap();
    assert_eq!(c.get(0).unwrap().name(), b"toyota_ae86.azcar");
    assert!(c.get(1).is_none());
}

#[test]
fn a_name_too_long_for_a_path_is_refused_rather_than_cut() {
    let mut c = Catalogue::EMPTY;
    let long = std::vec![b'a'; NAME_MAX + 1];
    assert_eq!(c.insert(&long), Err(Error::NameTooLong));
    assert!(c.is_empty());
    // One byte shorter is a name a path can be built from, so it is a car.
    assert_eq!(c.insert(&long[..NAME_MAX]), Ok(()));
    assert_eq!(c.len(), 1);
}

#[test]
fn the_table_fills_up_and_says_so() {
    let mut c = Catalogue::EMPTY;
    for i in 0..MAX_ENTRIES {
        c.insert(std::format!("car{i:04}.azcar").as_bytes())
            .unwrap();
    }
    // The one that does not fit is refused and nothing already filed is disturbed by it.
    assert_eq!(c.insert(b"one_too_many.azcar"), Err(Error::Full));
    assert_eq!(c.len(), MAX_ENTRIES);
    assert_eq!(c.get(0).unwrap().name(), b"car0000.azcar");
}

#[test]
fn a_car_is_named_by_its_filename_until_its_file_is_read() {
    let mut c = Catalogue::EMPTY;
    c.insert(b"nissan_s15.azcar").unwrap();
    c.insert(b"MERCEDES-190E.AZCAR").unwrap();
    assert_eq!(c.display_name(0).as_bytes(), b"MERCEDES 190E");
    assert_eq!(c.display_name(1).as_bytes(), b"NISSAN S15");
}

#[test]
fn a_display_name_for_a_car_that_is_not_there_is_empty() {
    let c = Catalogue::EMPTY;
    assert_eq!(c.display_name(0).as_bytes(), b"");
}

#[test]
fn only_azcar_files_are_offered() {
    assert!(is_car_file(b"nissan_s15.azcar"));
    assert!(is_car_file(b"NISSAN_S15.AZCAR"));
    assert!(is_car_file(b"a.AzCaR"));
    assert!(!is_car_file(b"notes.txt"));
    assert!(!is_car_file(b"azcar"));
    // An extension with no name in front of it.
    assert!(!is_car_file(b".azcar"));
    assert!(!is_car_file(b""));
}
