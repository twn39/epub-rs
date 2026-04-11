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

        let html =
            r#"<!DOCTYPE html><html><head><title>Test</title></head><body><p>1</p></body></html>"#;
        let mut output = Vec::new();

        let css = "<style>body { background: black; }</style>";
        inject_head_content(html.as_bytes(), &mut output, css).expect("Failed to inject CSS");

        let result = String::from_utf8(output).unwrap();
        assert!(
            result.contains("<title>Test</title><style>body { background: black; }</style></head>")
        );
    }

    #[test]
    fn test_search_chapter() {
        use epub_rs::processor::search_chapter;
        use regex::Regex;

        let html = r#"<!DOCTYPE html>
<html>
<body>
    <div id="content">
        <p>This is a story about a brave <b>knight</b> who fought a dragon.</p>
        <p>The knight was very brave.</p>
    </div>
</body>
</html>"#;

        let pattern = Regex::new(r"brave").unwrap();
        let results = search_chapter(html, "/6/4!", &pattern).expect("Search failed");

        assert_eq!(results.len(), 2);

        // First match in the first paragraph
        assert_eq!(results[0].excerpt, " is a story about a brave ");
        // path should be body(/4) -> div(/2[content]) -> p(/2) -> text(/1).
        // Note: the text node inside <p> is /1.
        assert_eq!(results[0].cfi, "epubcfi(/6/4!/4/2[content]/2,/1:24,/1:29)");

        // Second match in the second paragraph
        assert_eq!(results[1].excerpt, "The knight was very brave.");
        // path should be body(/4) -> div(/2[content]) -> p(/4) -> text(/1).
        assert_eq!(results[1].cfi, "epubcfi(/6/4!/4/2[content]/4,/1:20,/1:25)");
    }

    #[test]
    fn test_extract_positions() {
        use epub_rs::processor::extract_positions;

        let html = r#"<html><body><div id="content"><p>12345</p><p>67890</p><p>abcde</p></div></body></html>"#;

        let mut positions = Vec::new();
        let mut char_counter = 0;
        let mut global_pos = 0;

        // We set chars_per_position to 4.
        // First <p> is 5 chars ("12345"). It should emit a position after 4 chars.
        // Leftover: 1 char.
        // Second <p> is 5 chars ("67890"). Leftover (1) + 5 = 6. It should emit another position after 3 chars.
        // Leftover: 2 chars.
        // Third <p> is 5 chars ("abcde"). Leftover (2) + 5 = 7. It should emit another position after 2 chars.
        // Leftover: 3 chars.

        let ctx = epub_rs::processor::PositionContext {
            base_cfi: "/6/4!",
            chars_per_position: 4,
            spine_index: 0,
            href: "test.xhtml",
        };

        extract_positions(
            html,
            &ctx,
            &mut char_counter,
            &mut positions,
            &mut global_pos,
        );

        assert_eq!(positions.len(), 3);

        // Match 1: in "12345", at offset 4
        // path: body(/4) -> div(/2) -> p(/2) -> text(/1)
        assert_eq!(positions[0].cfi, "epubcfi(/6/4!/4/2[content]/2/1:4)");
        assert_eq!(positions[0].global_position, 1);

        // Match 2: in "67890", at offset 3
        // path: body(/4) -> div(/2) -> p(/4) -> text(/1)
        assert_eq!(positions[1].cfi, "epubcfi(/6/4!/4/2[content]/4/1:3)");
        assert_eq!(positions[1].global_position, 2);

        // Match 3: in "abcde", at offset 2
        // path: body(/4) -> div(/2) -> p(/6) -> text(/1)
        assert_eq!(positions[2].cfi, "epubcfi(/6/4!/4/2[content]/6/1:2)");
        assert_eq!(positions[2].global_position, 3);

        // Final leftover counter should be 3
        assert_eq!(char_counter, 3);
    }

    #[test]
    fn test_extract_semantic_content() {
        use epub_rs::processor::extract_semantic_content;

        let html = r#"<!DOCTYPE html>
<html lang="en">
<body>
    <div id="content">
        <h1>Chapter Title</h1>
        <p lang="fr">Bonjour!</p>
        <blockquote>Quote text</blockquote>
    </div>
</body>
</html>"#;

        let elements = extract_semantic_content(html, "/6/4[chap1]!");

        assert_eq!(elements.len(), 3);

        assert_eq!(elements[0].tag_name, "h1");
        assert_eq!(elements[0].text, "Chapter Title");
        assert_eq!(elements[0].language.as_deref(), Some("en")); // Inherited from html
        assert_eq!(
            elements[0].cfi_range,
            "epubcfi(/6/4[chap1]!/4/2[content]/2)"
        );

        assert_eq!(elements[1].tag_name, "p");
        assert_eq!(elements[1].text, "Bonjour!");
        assert_eq!(elements[1].language.as_deref(), Some("fr")); // Overridden
        assert_eq!(
            elements[1].cfi_range,
            "epubcfi(/6/4[chap1]!/4/2[content]/4)"
        );

        assert_eq!(elements[2].tag_name, "blockquote");
        assert_eq!(elements[2].text, "Quote text");
        assert_eq!(elements[2].language.as_deref(), Some("en")); // Inherited again
        assert_eq!(
            elements[2].cfi_range,
            "epubcfi(/6/4[chap1]!/4/2[content]/6)"
        );
    }
}
