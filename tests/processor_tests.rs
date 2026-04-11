#[cfg(test)]
mod tests {
    use epub_rs::processor::inject_cfi_dom;

    #[test]
    fn test_inject_cfi_dom() {
        let html = r#"<!DOCTYPE html>
<html>
<head>
    <title>Test</title>
</head>
<body>
    <div id="wrapper">
        <p>Paragraph 1</p>
        <p id="p2">Paragraph 2</p>
    </div>
</body>
</html>"#;

        let injected = inject_cfi_dom(html, "/6/4!").expect("Failed to inject CFIs");
        
        // Verify head
        assert!(injected.contains(r#"data-cfi="epubcfi(/6/4!/2)""#)); // <head>
        
        // Verify body and children
        assert!(injected.contains(r#"data-cfi="epubcfi(/6/4!/4)""#)); // <body>
        assert!(injected.contains(r#"data-cfi="epubcfi(/6/4!/4/2[wrapper])""#)); // <div id="wrapper">
        assert!(injected.contains(r#"data-cfi="epubcfi(/6/4!/4/2[wrapper]/2)""#)); // <p>
        assert!(injected.contains(r#"data-cfi="epubcfi(/6/4!/4/2[wrapper]/4[p2])""#)); // <p id="p2">
    }

    #[test]
    fn test_inject_head_content() {
        use epub_rs::processor::inject_head_content;
        
        let html = r#"<!DOCTYPE html><html><head><title>Test</title></head><body><p>1</p></body></html>"#;
        let mut output = Vec::new();
        
        let css = "<style>body { background: black; }</style>";
        inject_head_content(html.as_bytes(), &mut output, css).expect("Failed to inject CSS");
        
        let result = String::from_utf8(output).unwrap();
        assert!(result.contains("<title>Test</title><style>body { background: black; }</style></head>"));
    }
}
