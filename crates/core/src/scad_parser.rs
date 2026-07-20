use thiserror::Error;

use crate::{
    Argument, AstRoot, Expr, FunctionDefinition, GlobalVariable, ModuleDefinition, ModuleNode,
    Parameter,
};

#[derive(Debug, Error, PartialEq, Eq)]
#[error("OpenSCAD parse error at line {line}, column {column}: {message}")]
pub struct ScadParseError {
    pub line: usize,
    pub column: usize,
    pub message: String,
}

pub fn parse_scad(source: &str) -> Result<AstRoot, ScadParseError> {
    Parser::new(source).parse()
}

/// Extract declarations and include/use edges without interpreting executable module logic.
/// This is intended for dependency files used by completion and project bundling.
pub fn parse_scad_definitions(source: &str) -> Result<AstRoot, ScadParseError> {
    Parser::new(source).parse_definitions()
}

struct Parser<'a> {
    source: &'a str,
    pos: usize,
    next_id: usize,
}

impl<'a> Parser<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            pos: 0,
            next_id: 1,
        }
    }

    fn parse(mut self) -> Result<AstRoot, ScadParseError> {
        let mut ast = AstRoot::new();
        while self.skip_trivia()? {
            if self.consume_keyword("include") {
                ast.includes.push(self.parse_library_path()?);
            } else if self.consume_keyword("use") {
                ast.uses.push(self.parse_library_path()?);
            } else if self.consume_keyword("function") {
                ast.function_defines.push(self.parse_function()?);
            } else if self.consume_keyword("module") {
                ast.module_defines.push(self.parse_module_definition()?);
            } else if self.looks_like_assignment()? {
                ast.global_variables.push(self.parse_global()?);
            } else {
                ast.modules.push(self.parse_module_node()?);
            }
        }
        Ok(ast)
    }

    fn parse_definitions(mut self) -> Result<AstRoot, ScadParseError> {
        let mut ast = AstRoot::new();
        while self.skip_trivia()? {
            if self.consume_keyword("include") {
                ast.includes.push(self.parse_library_path()?);
            } else if self.consume_keyword("use") {
                ast.uses.push(self.parse_library_path()?);
            } else if self.consume_keyword("function") {
                ast.function_defines.push(self.parse_function()?);
            } else if self.consume_keyword("module") {
                ast.module_defines
                    .push(self.parse_shallow_module_definition()?);
            } else if self.looks_like_assignment()? {
                ast.global_variables.push(self.parse_global()?);
            } else {
                self.skip_executable_statement()?;
            }
        }
        Ok(ast)
    }

    fn parse_library_path(&mut self) -> Result<String, ScadParseError> {
        self.skip_trivia()?;
        self.expect_char('<')?;
        let start = self.pos;
        while let Some(ch) = self.peek() {
            if ch == '>' {
                let path = self.source[start..self.pos].trim().to_string();
                self.bump();
                self.skip_trivia()?;
                // OpenSCAD accepts both `include <file>` and `include <file>;` (likewise for
                // `use`). Most library code, including BOSL, uses the form without a semicolon.
                self.consume_char(';');
                return Ok(path);
            }
            self.bump();
        }
        Err(self.error("unterminated library path"))
    }

    fn parse_function(&mut self) -> Result<FunctionDefinition, ScadParseError> {
        self.skip_trivia()?;
        let name = self.identifier()?;
        let parameters = self.parse_parameters()?;
        self.skip_trivia()?;
        self.expect_char('=')?;
        let body = self.take_until_top_level(';')?;
        Ok(FunctionDefinition::new(name, parameters, parse_expr(&body)))
    }

    fn parse_module_definition(&mut self) -> Result<ModuleDefinition, ScadParseError> {
        self.skip_trivia()?;
        let name = self.identifier()?;
        let parameters = self.parse_parameters()?;
        self.skip_trivia()?;
        self.expect_char('{')?;
        let body = self.parse_module_body()?;
        Ok(ModuleDefinition::new(name, parameters, body))
    }

    fn parse_shallow_module_definition(&mut self) -> Result<ModuleDefinition, ScadParseError> {
        self.skip_trivia()?;
        let name = self.identifier()?;
        let parameters = self.parse_parameters()?;
        self.skip_trivia()?;
        let body_start = self.pos;
        self.skip_executable_statement()?;
        let body_source = &self.source[body_start..self.pos];
        let body = if body_source.contains("children(") || body_source.contains("children (") {
            vec![ModuleNode::new_leaf(
                self.node_id(),
                "children".to_string(),
                Vec::new(),
            )]
        } else {
            Vec::new()
        };
        Ok(ModuleDefinition::new(name, parameters, body))
    }

    fn skip_executable_statement(&mut self) -> Result<(), ScadParseError> {
        let start = self.pos;
        let mut parens = 0i32;
        let mut brackets = 0i32;
        let mut string = false;
        let mut escape = false;
        while let Some(ch) = self.peek() {
            if escape {
                escape = false;
            } else if string && ch == '\\' {
                escape = true;
            } else if ch == '"' {
                string = !string;
            } else if !string {
                match ch {
                    '(' => parens += 1,
                    ')' => parens -= 1,
                    '[' => brackets += 1,
                    ']' => brackets -= 1,
                    '{' if parens == 0 && brackets == 0 => {
                        self.take_balanced('{', '}')?;
                        return Ok(());
                    }
                    ';' if parens == 0 && brackets == 0 => {
                        self.bump();
                        return Ok(());
                    }
                    _ => {}
                }
            }
            self.bump();
        }
        if self.pos > start {
            Ok(())
        } else {
            Err(self.error("could not advance while scanning dependency definitions"))
        }
    }

    fn parse_parameters(&mut self) -> Result<Vec<Parameter>, ScadParseError> {
        self.skip_trivia()?;
        let content = self.take_balanced('(', ')')?;
        split_top_level(&content, ',')
            .into_iter()
            .filter(|part| !part.trim().is_empty())
            .map(|part| {
                if let Some((name, value)) = split_once_top_level(part, '=') {
                    Ok(Parameter::with_default(
                        name.trim().to_string(),
                        parse_expr(value.trim()),
                    ))
                } else {
                    Ok(Parameter::new(part.trim().to_string()))
                }
            })
            .collect()
    }

    fn parse_global(&mut self) -> Result<GlobalVariable, ScadParseError> {
        let name = self.identifier()?;
        self.skip_trivia()?;
        self.expect_char('=')?;
        let value = self.take_until_top_level(';')?;
        Ok(GlobalVariable::new(name, parse_expr(&value)))
    }

    fn parse_module_body(&mut self) -> Result<Vec<ModuleNode>, ScadParseError> {
        let mut nodes = Vec::new();
        loop {
            self.skip_trivia()?;
            if self.consume_char('}') {
                return Ok(nodes);
            }
            if self.eof() {
                return Err(self.error("unterminated module body"));
            }
            if self.looks_like_assignment()? {
                nodes.push(self.parse_raw_statement()?);
            } else {
                nodes.push(self.parse_module_node()?);
            }
        }
    }

    fn parse_raw_statement(&mut self) -> Result<ModuleNode, ScadParseError> {
        let statement = self.take_until_top_level(';')?;
        let mut node = ModuleNode::new_leaf(self.node_id(), "assignment".to_string(), Vec::new());
        node.display_name = Some(statement.trim().to_string());
        node.raw_statement = Some(format!("{};", statement.trim()));
        Ok(node)
    }

    fn parse_module_node(&mut self) -> Result<ModuleNode, ScadParseError> {
        self.skip_trivia()?;
        let modifier = if matches!(self.peek(), Some('#' | '%' | '*' | '!')) {
            let modifier = self.peek();
            self.bump();
            self.skip_trivia()?;
            modifier
        } else {
            None
        };
        let name = self.identifier()?;
        self.skip_trivia()?;
        let args = if self.peek() == Some('(') {
            let content = self.take_balanced('(', ')')?;
            parse_arguments(&content)
        } else {
            Vec::new()
        };
        let mut node = ModuleNode::new_leaf(self.node_id(), name, args);
        node.modifier = modifier;
        node.omit_parentheses = node.name == "else";
        self.skip_trivia()?;
        if self.consume_char(';') {
            return Ok(node);
        }
        if self.consume_char('{') {
            node.children = self.parse_module_body()?;
            return Ok(node);
        }

        // OpenSCAD permits a single child without braces: translate(...) cube(...);
        node.children.push(self.parse_module_node()?);
        Ok(node)
    }

    fn looks_like_assignment(&mut self) -> Result<bool, ScadParseError> {
        let saved = self.pos;
        self.skip_trivia()?;
        let result = self.identifier().is_ok() && {
            self.skip_trivia()?;
            self.peek() == Some('=') && !self.source[self.pos..].starts_with("==")
        };
        self.pos = saved;
        Ok(result)
    }

    fn node_id(&mut self) -> String {
        let id = format!("imported_{}", self.next_id);
        self.next_id += 1;
        id
    }

    fn identifier(&mut self) -> Result<String, ScadParseError> {
        let start = self.pos;
        if !matches!(self.peek(), Some(ch) if ch.is_alphabetic() || ch == '_' || ch == '$') {
            return Err(self.error("expected identifier"));
        }
        self.bump();
        while matches!(self.peek(), Some(ch) if ch.is_alphanumeric() || ch == '_') {
            self.bump();
        }
        Ok(self.source[start..self.pos].to_string())
    }

    fn take_balanced(&mut self, open: char, close: char) -> Result<String, ScadParseError> {
        self.expect_char(open)?;
        let start = self.pos;
        let mut depth = 1usize;
        let mut string = false;
        let mut escape = false;
        while let Some(ch) = self.peek() {
            if escape {
                escape = false;
                self.bump();
                continue;
            }
            if string && ch == '\\' {
                escape = true;
            } else if ch == '"' {
                string = !string;
            } else if !string && ch == open {
                depth += 1;
            } else if !string && ch == close {
                depth -= 1;
                if depth == 0 {
                    let content = self.source[start..self.pos].to_string();
                    self.bump();
                    return Ok(content);
                }
            }
            self.bump();
        }
        Err(self.error(&format!("unterminated {open}{close} group")))
    }

    fn take_until_top_level(&mut self, terminator: char) -> Result<String, ScadParseError> {
        let start = self.pos;
        let mut parens = 0i32;
        let mut brackets = 0i32;
        let mut string = false;
        let mut escape = false;
        while let Some(ch) = self.peek() {
            if escape {
                escape = false;
            } else if string && ch == '\\' {
                escape = true;
            } else if ch == '"' {
                string = !string;
            } else if !string {
                match ch {
                    '(' => parens += 1,
                    ')' => parens -= 1,
                    '[' => brackets += 1,
                    ']' => brackets -= 1,
                    _ if ch == terminator && parens == 0 && brackets == 0 => {
                        let result = self.source[start..self.pos].trim().to_string();
                        self.bump();
                        return Ok(result);
                    }
                    _ => {}
                }
            }
            self.bump();
        }
        Err(self.error(&format!("expected '{terminator}'")))
    }

    fn skip_trivia(&mut self) -> Result<bool, ScadParseError> {
        loop {
            while matches!(self.peek(), Some(ch) if ch.is_whitespace()) {
                self.bump();
            }
            if self.source[self.pos..].starts_with("//") {
                while !matches!(self.peek(), None | Some('\n')) {
                    self.bump();
                }
            } else if self.source[self.pos..].starts_with("/*") {
                self.pos += 2;
                if let Some(offset) = self.source[self.pos..].find("*/") {
                    self.pos += offset + 2;
                } else {
                    return Err(self.error("unterminated block comment"));
                }
            } else {
                break;
            }
        }
        Ok(!self.eof())
    }

    fn expect_char(&mut self, expected: char) -> Result<(), ScadParseError> {
        if self.consume_char(expected) {
            Ok(())
        } else {
            Err(self.error(&format!("expected '{expected}'")))
        }
    }

    fn consume_keyword(&mut self, keyword: &str) -> bool {
        if self.source[self.pos..].starts_with(keyword)
            && self.source[self.pos + keyword.len()..]
                .chars()
                .next()
                .is_none_or(|ch| !ch.is_alphanumeric() && ch != '_')
        {
            self.pos += keyword.len();
            true
        } else {
            false
        }
    }

    fn consume_char(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn peek(&self) -> Option<char> {
        self.source[self.pos..].chars().next()
    }

    fn bump(&mut self) {
        if let Some(ch) = self.peek() {
            self.pos += ch.len_utf8();
        }
    }

    fn eof(&self) -> bool {
        self.pos >= self.source.len()
    }

    fn error(&self, message: &str) -> ScadParseError {
        let prefix = &self.source[..self.pos];
        let line = prefix.bytes().filter(|byte| *byte == b'\n').count() + 1;
        let column = prefix
            .rsplit('\n')
            .next()
            .unwrap_or_default()
            .chars()
            .count()
            + 1;
        ScadParseError {
            line,
            column,
            message: message.to_string(),
        }
    }
}

fn parse_arguments(content: &str) -> Vec<Argument> {
    split_top_level(content, ',')
        .into_iter()
        .filter(|part| !part.trim().is_empty())
        .map(|part| {
            if let Some((name, value)) = split_once_top_level(part, '=') {
                Argument::Named {
                    name: name.trim().to_string(),
                    value: parse_expr(value.trim()),
                }
            } else {
                Argument::Positional(parse_expr(part.trim()))
            }
        })
        .collect()
}

fn parse_expr(source: &str) -> Expr {
    Expr::parse(source).unwrap_or_else(|_| Expr::Raw(source.to_string()))
}

fn split_once_top_level(source: &str, separator: char) -> Option<(&str, &str)> {
    top_level_separator_positions(source, separator)
        .into_iter()
        .next()
        .map(|position| {
            (
                &source[..position],
                &source[position + separator.len_utf8()..],
            )
        })
}

fn split_top_level(source: &str, separator: char) -> Vec<&str> {
    let positions = top_level_separator_positions(source, separator);
    let mut result = Vec::new();
    let mut start = 0;
    for position in positions {
        result.push(&source[start..position]);
        start = position + separator.len_utf8();
    }
    result.push(&source[start..]);
    result
}

fn top_level_separator_positions(source: &str, separator: char) -> Vec<usize> {
    let mut positions = Vec::new();
    let (mut parens, mut brackets, mut braces) = (0i32, 0i32, 0i32);
    let mut string = false;
    let mut escape = false;
    for (position, ch) in source.char_indices() {
        if escape {
            escape = false;
            continue;
        }
        if string && ch == '\\' {
            escape = true;
        } else if ch == '"' {
            string = !string;
        } else if !string {
            match ch {
                '(' => parens += 1,
                ')' => parens -= 1,
                '[' => brackets += 1,
                ']' => brackets -= 1,
                '{' => braces += 1,
                '}' => braces -= 1,
                _ if ch == separator && parens == 0 && brackets == 0 && braces == 0 => {
                    positions.push(position)
                }
                _ => {}
            }
        }
    }
    positions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_complete_structured_document() {
        let ast = parse_scad(
            r#"
                include <parts/common.scad>;
                use <lib/shapes.scad>;
                $fn = 48;
                size = [10, 20, 30];
                function twice(x) = x * 2;
                module post(r=2, h) {
                    local = h / 2;
                    translate([0, 0, local]) cylinder(r=r, h=h);
                }
                difference() {
                    cube(size, center=true);
                    sphere(r=twice(3));
                }
            "#,
        )
        .unwrap();

        assert_eq!(ast.includes, ["parts/common.scad"]);
        assert_eq!(ast.uses, ["lib/shapes.scad"]);
        assert_eq!(ast.global_variables.len(), 2);
        assert_eq!(ast.function_defines[0].name, "twice");
        assert_eq!(ast.module_defines[0].name, "post");
        assert_eq!(ast.module_defines[0].body.len(), 2);
        assert_eq!(ast.module_defines[0].body[1].children[0].name, "cylinder");
        assert_eq!(ast.modules[0].name, "difference");
        assert_eq!(ast.modules[0].children.len(), 2);
    }

    #[test]
    fn parses_library_directives_with_or_without_semicolons() {
        let ast = parse_scad(
            "include <BOSL/constants.scad>\nuse <BOSL/transforms.scad>\ninclude <local.scad>;\ncube(1);",
        )
        .unwrap();

        assert_eq!(ast.includes, ["BOSL/constants.scad", "local.scad"]);
        assert_eq!(ast.uses, ["BOSL/transforms.scad"]);
        assert_eq!(ast.modules.len(), 1);
    }

    #[test]
    fn definition_scan_accepts_single_statement_module_bodies() {
        let ast = parse_scad_definitions(
            "module right(x=0) translate([x,0,0]) children();\nmodule solid() cube(1);",
        )
        .unwrap();

        assert_eq!(ast.module_defines.len(), 2);
        assert_eq!(ast.module_defines[0].name, "right");
        assert_eq!(ast.module_defines[0].body[0].name, "children");
        assert_eq!(ast.module_defines[1].name, "solid");
        assert!(ast.module_defines[1].body.is_empty());
    }

    #[test]
    fn preserves_unsupported_expressions_as_raw_ast_values() {
        let ast = parse_scad("points = [for (x = [0:2]) [x, x*x]]; polygon(points);").unwrap();
        assert!(matches!(ast.global_variables[0].value, Expr::Raw(_)));
        assert!(ast.to_scad().contains("[for (x = [0:2]) [x, x*x]]"));
    }

    #[test]
    fn reports_location_for_invalid_input() {
        let error = parse_scad("cube(1);\nmodule broken() {").unwrap_err();
        assert_eq!(error.line, 2);
        assert!(error.message.contains("unterminated module body"));
    }

    #[test]
    fn definition_scan_ignores_unsupported_executable_logic() {
        let ast = parse_scad_definitions(
            r#"
                include <nested.scad>;
                weird syntax that the structured parser does not support { anything goes; }
                module gear(teeth=20) { children(); unsupported ?? logic; }
                function pitch(d, teeth) = d / teeth;
            "#,
        )
        .unwrap();
        assert_eq!(ast.includes, ["nested.scad"]);
        assert_eq!(ast.module_defines[0].name, "gear");
        assert_eq!(ast.function_defines[0].name, "pitch");
        assert!(ast.module_defines[0]
            .body
            .iter()
            .any(|node| node.name == "children"));
    }
}
