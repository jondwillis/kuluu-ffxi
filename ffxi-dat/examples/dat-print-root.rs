fn main() {
    let r = ffxi_dat::DatRoot::from_env_or_default().unwrap();
    println!("DAT_ROOT={}", r.root().display());
}
