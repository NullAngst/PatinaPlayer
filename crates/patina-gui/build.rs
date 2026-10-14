fn main() {
    // fluent-dark only styles the scroll bars we inherit from std-widgets;
    // everything else is drawn in the Runout palette by ui/app.slint.
    let config = slint_build::CompilerConfiguration::new().with_style("fluent-dark".into());
    slint_build::compile_with_config("ui/app.slint", config).expect("compile ui/app.slint");
}
