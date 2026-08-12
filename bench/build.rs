fn main() {
    pkg_config::Config::new()
        .probe("libbrotlienc")
        .expect("libbrotlienc not found through pkg-config; install libbrotli-dev");
    pkg_config::Config::new()
        .probe("libbrotlidec")
        .expect("libbrotlidec not found through pkg-config; install libbrotli-dev");
}
