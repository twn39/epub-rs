use lol_html::{text, HtmlRewriter, Settings};
use std::cell::RefCell;
use std::rc::Rc;

fn main() {
    let html = br#"<html><body><h1>Title</h1><p>Hello <b>World</b>!</p></body></html>"#;
    let extracted_text = Rc::new(RefCell::new(String::new()));
    let text_clone = Rc::clone(&extracted_text);

    let mut rewriter = HtmlRewriter::new(
        Settings {
            element_content_handlers: vec![
                text!("body", |t| {
                    text_clone.borrow_mut().push_str(t.as_str());
                    Ok(())
                })
            ],
            ..Settings::default()
        },
        |_: &[u8]| {}
    );

    rewriter.write(html).unwrap();
    rewriter.end().unwrap();

    println!("Extracted: {}", extracted_text.borrow());
}
