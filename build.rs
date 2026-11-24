//! Embarque l'icône et les métadonnées dans l'exe Windows.

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/icon.ico")
        .set("ProductName", "Snappy")
        .set("FileDescription", "Snappy")
        .set("LegalCopyright", "MIT");
    if let Err(e) = res.compile() {
        // Sans ressource, l'exe fonctionne quand même (icône par défaut).
        println!("cargo:warning=ressources Windows non compilées : {e}");
    }
}
