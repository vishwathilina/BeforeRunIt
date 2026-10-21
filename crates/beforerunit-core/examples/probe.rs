fn main() {
    for l in std::env::args().skip(1) {
        let a = beforerunit_core::analyze(&l);
        println!("{:<50} {:?} hard={} root={} complex={} | {}", l, a.level, a.hard, a.root, a.complex, a.text);
    }
}
