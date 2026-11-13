use crate::numeric::{LAMPORTS_PER_SOL, lamports_to_sol};

#[test]
fn should_have_one_billion_lamports_per_sol() {
    assert_eq!(LAMPORTS_PER_SOL, 1_000_000_000);
}

#[test]
fn should_format_lamports_as_sol() {
    for (lamports, expected) in [
        (0, "0"),
        (1, "0.000000001"),
        (10, "0.00000001"),
        (2_000_000, "0.002"),
        (999_999_999, "0.999999999"),
        (1_000_000_000, "1"),
        (1_500_000_000, "1.5"),
        (1_000_000_001, "1.000000001"),
        (42_000_000_000, "42"),
        (u64::MAX, "18446744073.709551615"),
    ] {
        assert_eq!(lamports_to_sol(lamports), expected, "{lamports} lamports");
    }
}
