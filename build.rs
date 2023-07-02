fn main() {
    embed_resource::compile("persistentwin.rc", embed_resource::NONE);

    let mut config = vergen::Config::default();
    *config.git_mut().sha_kind_mut() = vergen::ShaKind::Short;

    vergen::vergen(config).expect("failed to generate version information");
}
