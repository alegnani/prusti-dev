extern crate prusti_contracts;

// ignore-test We need to restore permissions when a mutable borrow dies

struct T(u32);

fn random() -> bool { true }

fn consume(y: &mut T) { }

fn check_join(mut x: (T, T)) -> T {
    // We have both `x.0` and `x.1`
    if random() {
        // We move out `x.0`
        consume(&mut x.0)
    }
    // After the join we would like to use `x.1`
    x.1
}

fn main() {}
