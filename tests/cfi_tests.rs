#[cfg(test)]
mod tests {
    use epub_rs::cfi::{CfiStep, EpubCfi};
    use std::str::FromStr;

    #[test]
    fn test_cfi_parsing() {
        let cfi_str = "epubcfi(/6/4[chap01ref]!/4[body01]/10[para05]/2/1:3)";
        
        let cfi = EpubCfi::from_str(cfi_str).expect("Failed to parse CFI");
        
        match cfi {
            EpubCfi::Point(path) => {
                // Check base path
                assert_eq!(path.steps.len(), 2);
                assert_eq!(path.steps[0].index, 6);
                assert_eq!(path.steps[0].assertion, None);
                assert_eq!(path.steps[1].index, 4);
                assert_eq!(path.steps[1].assertion, Some("chap01ref".to_string()));
                
                // Check local path
                let local = path.local_steps.expect("Missing local steps");
                assert_eq!(local.len(), 4);
                assert_eq!(local[0].index, 4);
                assert_eq!(local[0].assertion, Some("body01".to_string()));
                assert_eq!(local[1].index, 10);
                assert_eq!(local[1].assertion, Some("para05".to_string()));
                assert_eq!(local[2].index, 2);
                assert_eq!(local[2].assertion, None);
                assert_eq!(local[3].index, 1);
                assert_eq!(local[3].assertion, None);
                
                // Check character offset
                assert_eq!(path.character_offset, Some(3));
            },
            _ => panic!("Expected Point CFI"),
        }
    }

    #[test]
    fn test_cfi_range_parsing() {
        let cfi_str = "epubcfi(/6/4[chap01ref]!/4[body01]/10[para05],/2/1:1,/3:4)";
        let cfi = EpubCfi::from_str(cfi_str).expect("Failed to parse range CFI");
        
        match cfi {
            EpubCfi::Range { parent, start, end } => {
                assert_eq!(parent.steps.len(), 2); // /6/4
                assert_eq!(parent.local_steps.as_ref().unwrap().len(), 2); // /4/10
                
                assert_eq!(start.steps.len(), 2); // /2/1
                assert_eq!(start.character_offset, Some(1));
                
                assert_eq!(end.steps.len(), 1); // /3
                assert_eq!(end.character_offset, Some(4));
            },
            _ => panic!("Expected Range CFI"),
        }
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
