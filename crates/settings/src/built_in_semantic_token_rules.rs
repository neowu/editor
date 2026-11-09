use settings_content::SemanticTokenRule;
use settings_content::SemanticTokenRules;

/// Fallback mappings, applied below language-specific rules.
/// Earlier rules take precedence; style names are tried in order.
pub(crate) fn rules() -> SemanticTokenRules {
    SemanticTokenRules {
        rules: vec![
            rule("enumMember", &[], &["type"]),
            rule("namespace", &[], &["variable"]),
            rule("namespace", &[], &["namespace", "module", "type"]),
            rule("class", &["declaration"], &["type.class.definition", "type.definition"]),
            rule("class", &["definition"], &["type.class.definition", "type.definition"]),
            rule("class", &[], &["type.class", "class", "type"]),
            rule("enum", &["declaration"], &["type.enum.definition", "type.definition"]),
            rule("enum", &["definition"], &["type.enum.definition", "type.definition"]),
            rule("enum", &[], &["type.enum", "enum", "type"]),
            rule("interface", &["declaration"], &["type.interface.definition", "type.definition"]),
            rule("interface", &["definition"], &["type.interface.definition", "type.definition"]),
            rule("interface", &[], &["type.interface", "interface", "type"]),
            rule("struct", &["declaration"], &["type.struct.definition", "type.definition"]),
            rule("struct", &["definition"], &["type.struct.definition", "type.definition"]),
            rule("struct", &[], &["type.struct", "struct", "type"]),
            rule("typeParameter", &["declaration"], &["type.parameter.definition", "type.definition"]),
            rule("typeParameter", &["definition"], &["type.parameter.definition", "type.definition"]),
            rule("typeParameter", &[], &["type.parameter", "type"]),
            rule("type", &["declaration"], &["type.definition"]),
            rule("type", &["definition"], &["type.definition"]),
            rule("type", &[], &["type"]),
            rule("parameter", &[], &["variable.parameter"]),
            rule("variable", &["defaultLibrary", "constant"], &["constant.builtin"]),
            rule("variable", &["defaultLibrary"], &["variable.builtin"]),
            rule("variable", &["constant"], &["constant"]),
            rule("variable", &[], &["variable"]),
            rule("property", &[], &["property"]),
            rule("enumMember", &[], &["type.enum.member", "type.enum", "variant"]),
            rule("decorator", &[], &["function.decorator", "function.annotation", "attribute"]),
            rule("function", &["defaultLibrary"], &["function.builtin"]),
            rule("function", &[], &["function"]),
            rule("method", &["defaultLibrary"], &["function.builtin"]),
            rule("method", &[], &["function.method", "function"]),
            rule("macro", &[], &["function.macro", "function"]),
            rule("label", &[], &["label"]),
            rule("comment", &["documentation"], &["comment.documentation", "comment.doc"]),
            rule("comment", &[], &["comment"]),
            rule("string", &["documentation"], &["string.doc"]),
            rule("string", &[], &["string"]),
            rule("keyword", &[], &["keyword"]),
            rule("number", &[], &["number"]),
            rule("regexp", &[], &["string.regexp", "string"]),
            rule("operator", &[], &["operator"]),
            rule("modifier", &[], &["keyword.modifier"]),
            rule("event", &[], &["type.event", "type"]),
        ],
    }
}

fn rule(token_type: &str, modifiers: &[&str], styles: &[&str]) -> SemanticTokenRule {
    SemanticTokenRule {
        token_type: Some(token_type.into()),
        token_modifiers: modifiers.iter().map(|modifier| (*modifier).into()).collect(),
        style: styles.iter().map(|style| (*style).into()).collect(),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn personal_rules_take_precedence() {
        let rules = rules().rules;
        let first_style = |token_type: &str| {
            rules.iter().find(|rule| rule.token_type.as_deref() == Some(token_type)).map(|rule| rule.style.clone())
        };
        assert_eq!(first_style("enumMember"), Some(vec!["type".to_string()]));
        assert_eq!(first_style("namespace"), Some(vec!["variable".to_string()]));
    }
}
