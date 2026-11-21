pub(crate) struct Identifier {
    pub(crate) formatted: String,
    pub(crate) compact: String,
}

impl Identifier {
    pub(crate) fn parse(query: &str) -> Result<Self, String> {
        let formatted = query
            .trim()
            .to_ascii_uppercase()
            .replace(['－', '–', '—', '−', '_'], "-");
        if formatted.len() > 64
            || !formatted.bytes().any(|byte| byte.is_ascii_digit())
            || formatted.split('-').any(|part| {
                part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_alphanumeric())
            })
        {
            return Err("请输入有效的番号或商品 ID，例如 ABC-123。".into());
        }
        let compact = compact_id(&formatted);
        Ok(Self { formatted, compact })
    }

    pub(crate) fn dvd_candidate(&self) -> String {
        if !self.formatted.contains('-') {
            let prefix_len = self
                .formatted
                .bytes()
                .take_while(u8::is_ascii_alphabetic)
                .count();
            if prefix_len > 0
                && self.formatted[prefix_len..]
                    .bytes()
                    .all(|byte| byte.is_ascii_digit())
            {
                let (prefix, digits) = self.formatted.split_at(prefix_len);
                return format!("{prefix}-{digits}");
            }
        }
        self.formatted.clone()
    }

    pub(crate) fn matches(&self, value: &str) -> bool {
        self.compact == compact_id(value)
    }
}

fn compact_id(value: &str) -> String {
    value
        .replace(['-', '_', '－', '–', '—', '−'], "")
        .to_ascii_lowercase()
}
