fn main() {
    embed_resource::compile("assets/syner-uart-recorder.rc", embed_resource::NONE)
        .manifest_optional()
        .unwrap();
}
