//! Development probe: keep, read back and remove a test secret in the
//! keyring (`secret_probe`), to check the Secret Service client.
fn main() {
    use narrivy_platform_linux::secrets;
    println!("store: {:?}", secrets::store("probe-test", "Narrivy probe (test)", "hello"));
    println!("read: {:?}", secrets::read("probe-test").map(|v| v.map(|s| s.len())));
    println!("remove: {:?}", secrets::remove("probe-test"));
    println!("read after: {:?}", secrets::read("probe-test"));
}
