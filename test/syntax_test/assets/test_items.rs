fn prod() {
    helper_prod();
}

#[cfg(not(test))]
fn not_test() {
    helper_not_test();
}

#[cfg(any(test, feature = "test-support"))]
pub fn support() {
    helper_support();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() {
        helper_fixture();
    }

    #[test]
    // A comment between the attributes and the item.
    #[should_panic]
    fn case() {
        assert_eq!(helper_macro(), 1);
    }
}
