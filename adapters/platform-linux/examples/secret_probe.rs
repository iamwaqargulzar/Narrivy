//! Development probe: keep, read back and remove a test secret in the
//! keyring (`secret_probe`), to check the Secret Service client.
// A Linux-only probe: other targets build it as an empty program.
#![cfg_attr(not(target_os = "linux"), no_main)]
#![cfg(target_os = "linux")]

fn main() {
    use narrivy_platform_linux::secrets;
    println!(
        "store: {:?}",
        secrets::store("probe-test", "Narrivy probe (test)", "hello")
    );
    println!(
        "read: {:?}",
        secrets::read("probe-test").map(|v| v.map(|s| s.len()))
    );
    println!("remove: {:?}", secrets::remove("probe-test"));
    println!("read after: {:?}", secrets::read("probe-test"));
}
