#[cfg(test)]
mod tests {
    use epub_rs::cfi::{CfiStep, EpubCfi};
    use std::str::FromStr;

    #[test]
    fn test_cfi_parsing() {
        let cfi_str = "epubcfi(/6/4[chap01ref]!/4[body01]/10[para05]/2/1:3)";
        
        let cfi = EpubCfi::from_str(cfi_str).expect("Failed to parse CFI");
        
        // Check base path
        assert_eq!(cfi.base_path.len(), 2);
        assert_eq!(cfi.base_path[0].index, 6);
        assert_eq!(cfi.base_path[0].assertion, None);
        assert_eq!(cfi.base_path[1].index, 4);
        assert_eq!(cfi.base_path[1].assertion, Some("chap01ref".to_string()));
        
        // Check local path
        assert_eq!(cfi.local_path.len(), 4);
        assert_eq!(cfi.local_path[0].index, 4);
        assert_eq!(cfi.local_path[0].assertion, Some("body01".to_string()));
        assert_eq!(cfi.local_path[1].index, 10);
        assert_eq!(cfi.local_path[1].assertion, Some("para05".to_string()));
        assert_eq!(cfi.local_path[2].index, 2);
        assert_eq!(cfi.local_path[2].assertion, None);
        assert_eq!(cfi.local_path[3].index, 1);
        assert_eq!(cfi.local_path[3].assertion, None);
        
        // Check character offset
        assert_eq!(cfi.character_offset, Some(3));
    }

    #[test]
    fn test_cfi_generation() {
        let cfi = EpubCfi::new()
            .add_base_step(CfiStep::new(6, None))
            .add_base_step(CfiStep::new(4, Some("chap01ref".to_string())))
            .add_local_step(CfiStep::new(4, Some("body01".to_string())))
            .add_local_step(CfiStep::new(10, Some("para05".to_string())))
            .add_local_step(CfiStep::new(2, None))
            .add_local_step(CfiStep::new(1, None))
            .character_offset(3);

        let generated_str = cfi.to_string();
        assert_eq!(generated_str, "epubcfi(/6/4[chap01ref]!/4[body01]/10[para05]/2/1:3)");
    }
}
