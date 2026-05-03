pub(super) fn tool_wrapper_symbol(tool_name: &str) -> String {
    let mangled: String = tool_name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("__corvid_tool_{mangled}")
}
