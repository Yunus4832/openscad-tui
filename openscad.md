# 背景知识

我现在需要使用 rust 完成一个代码生成的项目，主要功能是生成 OpenSCAD 的代码，下面是一份 OpenSCAD 的 CST 具体语法树的定义文件

```ungram
// Grammar for the OpenSCAD programming language.
//
// For information on how this file is used, see the original article from
// rust-analyzer:
//
// https://rust-analyzer.github.io/blog/2020/10/24/introducing-ungrammar.html

Package = Statement*

Statement =
  Include
| Use
| AssignmentStatement
| NamedFunctionDefinition
| NamedModuleDefinition
| ModuleInstantiation
| IfStatement
| ForStatement

Include = 'include' 'file'

Use = 'use' 'file'

IfStatement = 'if' '(' Expr ')' Actions ( 'else' Actions )?
ForStatement = 'for' '(' Assignments ')' Actions

Actions =
  Action
| BracedActions

BracedActions = '{' Action* '}'

Action =
  AssignmentStatement
| ModuleInstantiation
| IfStatement
| ForStatement

AssignmentStatement = Assignment ';'

Expr =
  Atom
| ListExpr
| RangeExpr
| UnaryExpr
| TernaryExpr
| ParenExpr
| ListComprehensionExpr
| BinExpr

Atom =
  LiteralExpr
| LookupExpr
| IndexExpr
| FunctionCall

LiteralExpr = 'true' | 'false' | 'undef' | 'integer' | 'float' | 'string'

LookupExpr = 'ident' ('.' 'ident')*

TernaryExpr = Expr '?' Expr ':' Expr

IndexExpr = Expr '[' Expr ']'

ParenExpr = '(' Expr ')'

ListComprehensionExpr = '[' ForClause ']'

BinExpr = Expr (BinOp Expr)*

BinOp = '+' | '-' | '*' | '/' | '%' | '^' | '>=' | '>' | '==' | '!=' | '<=' | '<' | '&&' | '||'

UnaryExpr = UnaryOp Expr

UnaryOp = '!' | '+' | '-'

RangeExpr = RangeExprFromTo | RangeExprFromToStep

RangeExprFromTo = '[' Expr ':' Expr ']'

RangeExprFromToStep = '[' Expr ':' Expr ':' Expr ']'

ListExpr = '[' Expr (',' Expr)* ']'

ListComprehensionElement =
  LetClause
| ForClause
| IfClause

ListComprehensionElementOrExpr = ListComprehensionElement | Expr

LetClause = 'let' '(' assignments:AssignmentsOpt ')' ListComprehensionElementOrExpr

ForClause = 'for' '(' assignments:Assignments ')' ListComprehensionElementOrExpr

IfClause = 'if' '(' condition:Expr ')' ListComprehensionElementOrExpr

NamedFunctionDefinition = 'function' 'ident' '(' params:Parameters? ')' '=' body:Expr ';'

NamedModuleDefinition = 'module' 'ident' '(' params:Parameters? ')' '{' body:Statement* '}'

FunctionCall = 'ident' '(' args:Arguments? ')'

ModuleInstantiation = 'ident' '(' args:Arguments? ')' Child

Children = Child*

Child =
  ';'
| BracedChildren
| ModuleInstantiation

BracedChildren = '{' Children '}'

Parameters = Parameter (',' Parameter)*

Parameter = variable:'ident' | Assignment

Arguments = Argument (',' Argument)*

Argument = Expr | Assignment

AssignmentsOpt = Assignments?

Assignments = Assignment (',' Assignment)*

Assignment = name:'ident' '=' value:Expr

```

这份文件仅仅用于参考 OpenSCAD 的语法，项目中不会使用 CST 因为 CST 更擅长于分析代码，而不是生成代码，项目中真正使用的
是自定义的 AST 抽象语法树。

# 需求描述

OpenSCAD 源文件虽然也是代码，但是文件中大部分是样板代码的各种嵌套组合，有时如果想要将两个模块通过布尔操作例如 difference 时需要
同时使用 `{}` 包裹两个模块，这往往是繁琐不方便的。

因此，我想设计一个基于命令的 OpenSCAD 包装噐，实现了类似 AutoCAD 的类似体验，选中，输入命令，输入参数，选中模块，确认完成操作，不同之处在于，
该包装噐生成的是代码。

例如 difference 命令会将下面代码中的两个 `cube` 模块求差集。

执行命令前:
```SCAD
cube([10,10,10], center=false);
cube([10,10,10], center=true);
```

执行命令后:
```SCAD
difference(){
    cube([10,10,10], center=false);
    cube([10,10,10], center=true);
}
```

- 例如 insert 命令可以插入模块，cube、square 等模块；
- 例如 select 命令可以选中需要操作的模块；
- 例如 translate 命令可以移动模块等等；

支持导入和导出 OpenSCAD 的库，导入库之后，可以在 insert 命令中候选菜单中看到库中的模块，并且在 insert 之后生成的代码自动
include 该库。

交互设计如下：

- 存在一棵 OpenSCAD 结构的树形目录，映射生成的 OpenSCAD 代码；
- 存在一个底部的输入框，用于输入命令以及参数等；
- 存在一个预览窗口，展示生成的代码；
- 提供一些内建的模块，例如 cube、square、等，这些 OpenSCAD 原生提供的基础模块；
- 使用类似 Vim 的交互方式，jkhl 分别绑定到树上下移动，展开和收起节点;
- i 绑定到 insert 命令，默认在下方插入实体；
- a 绑定到 insert 命令，默认在上方插入实体；
- insert 命令提供一个候选列表，数据源来自内建模块和库加载，如果模块带有参数，候选列表应该能够提示用户模块的参数格式；
- insert 命令支持通过输入筛选需要的模块，回车确认；
- insert 命令选中的模块如果有参数，则需要输入参数，在底部的输入框中回显；
- insert 命令输入模块参数时， 例如 cube([10,10,10], center=true) 需要用户输入 `[10,10,10], true` 解析参数并插入到树中，如果参数解析错误，提示用户参数格式；
- insert 命令选中的模块如果需要操作 children，这在 OpenSCAD 中以为中它可以包含子模块，如果当前用户没有选中任何子模块，提示用户选取子模块，如果执行命令前已经选中子模块，则直接使用选中的子模块；
- dd/D 绑定到 delete 命令，删除当前光标下的模块；
- x 绑定到 remove 命令，只删除树中当前节点，并将叶子节点移动到父节点；
- : 绑定到 command 命令，进入命令模式，可以输入合法的命令，并在底部的输入框回显 enter 执行命令
- y 绑定到 yank 命令，复制节点；
- p 绑定到 paste 命令，在下方粘贴节点；
- v 绑定到 select 命令，使用 jk 移动选中多个模块；
- u 绑定到 undo 命令，撤销上一个操作；
- r 绑定到 replace 命令，将当前实体替换成其他模块；
- q 绑定到 quit 命令，退出应用；
- w 绑定到 write 命令，将当前操作的树序列化到指定 yaml 文件，方便再次编辑；
- e 绑定到 edit 命令，加载指定 yaml 文件，重建树形目录，继续编辑；
- export 命令，将生成的预览 OpenSCAD 代码导出到指定文件；
- load 命令，导入 OpenSCAD 库定义文件并且合并到内建的库作为 insert 命令的候选列表；
- 实现布尔操作的模块，例如 difference union 由于它十分常用，并且是内建的模块，将其实现为命令，可以直接在命令模式中使用；

# 功能模块设计

这里的模块和 OpenSCAD 中的模块不是同一个概念，这一节中指的项目功能模块的拆分，项目设想拆分成以下几个功能模块，职责不同。

##  核心模块 OpenSCAD AST API

该模块是项目的底层核心，提供了一组 API 负责构建和操作 AST 抽象语法树。所以的命令都将通过这组 API 实现，API 提供的核心能力包括

- 创建空的 AST 语法树;
- 插入/删除子节点;
- 替换子节点;
- 修改子节点;
- 序列化/反序列化 AST 语法树;

该 AST 的设计也是项目的核心，该 AST 需要根据 OpenSCAD 的语法进行设计，以方便后续代码生成。

## OpenSCAD 库管理模块

该模块主要功能是提供项目内可用的模块，加载第三方 OpenSCAD 库的定义文件，并生成模块列表；

主要的实现逻辑的核心是 OpenSCAD 库定义文件，该文件也是一个自定义的 yaml 文件，需要库作者提供，或者用户解析库并
自行创建，它描述了

- 库名称，用于生成代码时 `include` 库；
- 库中所有的模块定义和参数列表；
- 是否包含 children 子模块的操作，该信息用于在交互上提示用户是否需要选择子模块进行操作；
- 提供 API 获取当前上下文可用的模块列表，列表项的数据结构需要和内建模块的数据结构一致，因为需要和内建模块在运行时进行合并提供给 insert 命令使用；

## UI 模块

该模块是用户交互模块，主要功能如下。

- 提供命令和绑定让用户方便操作语法树；
- 支持将 AST 语法树导出为 OpenSCAD 文件；
- 支持同时也支持将当前工作也就是 AST 语法树保存为 yaml 文件；
- 支持加载保存的 yaml 文件继续之前的工作；

# 代码设计

- 使用 Rust 实现；
- 不同功能模块封装到不同的 crate；
- UI 交互使用 ratatui 实现；
- 使用 cargo 创建项目并安装依赖；
- 项目代号为 openscad-tui；
- 使用 git 管理代码仓库，创建合适的 .gitignore；

项目需要使用到 tui_tree_widget 模块，可以直接使用 cargo 安装，参考源码如下：

```rust
//! Widget built to show Tree Data structures.
//!
//! Tree widget [`Tree`] is generated with [`TreeItem`]s (which itself can contain [`TreeItem`] children to form the tree structure).
//! The user interaction state (like the current selection) is stored in the [`TreeState`].

use std::collections::HashSet;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{Block, Scrollbar, ScrollbarState, StatefulWidget, Widget};
use unicode_width::UnicodeWidthStr as _;

pub use crate::flatten::Flattened;
pub use crate::tree_item::TreeItem;
pub use crate::tree_state::TreeState;

mod flatten;
mod tree_item;
mod tree_state;

/// A `Tree` which can be rendered.
///
/// The generic argument `Identifier` is used to keep the state like the currently selected or opened [`TreeItem`]s in the [`TreeState`].
/// For more information see [`TreeItem`].
///
/// # Example
///
/// ```
/// # use tui_tree_widget::{Tree, TreeItem, TreeState};
/// # use ratatui::backend::TestBackend;
/// # use ratatui::Terminal;
/// # use ratatui::widgets::Block;
/// # let mut terminal = Terminal::new(TestBackend::new(32, 32)).unwrap();
/// let mut state = TreeState::default();
///
/// let item = TreeItem::new_leaf("l", "leaf");
/// let items = vec![item];
///
/// terminal.draw(|frame| {
///     let area = frame.size();
///
///     let tree_widget = Tree::new(&items)
///         .expect("all item identifiers are unique")
///         .block(Block::bordered().title("Tree Widget"));
///
///     frame.render_stateful_widget(tree_widget, area, &mut state);
/// })?;
/// # Ok::<(), std::io::Error>(())
/// ```
#[must_use]
#[derive(Debug, Clone)]
pub struct Tree<'a, Identifier> {
    items: &'a [TreeItem<'a, Identifier>],

    block: Option<Block<'a>>,
    scrollbar: Option<Scrollbar<'a>>,
    /// Style used as a base style for the widget
    style: Style,

    /// Style used to render selected item
    highlight_style: Style,
    /// Symbol in front of the selected item (Shift all items to the right)
    highlight_symbol: &'a str,

    /// Symbol displayed in front of a closed node (As in the children are currently not visible)
    node_closed_symbol: &'a str,
    /// Symbol displayed in front of an open node. (As in the children are currently visible)
    node_open_symbol: &'a str,
    /// Symbol displayed in front of a node without children.
    node_no_children_symbol: &'a str,
}

impl<'a, Identifier> Tree<'a, Identifier>
where
    Identifier: Clone + PartialEq + Eq + core::hash::Hash,
{
    /// Create a new `Tree`.
    ///
    /// # Errors
    ///
    /// Errors when there are duplicate identifiers in the children.
    pub fn new(items: &'a [TreeItem<'a, Identifier>]) -> std::io::Result<Self> {
        let identifiers = items
            .iter()
            .map(|item| &item.identifier)
            .collect::<HashSet<_>>();
        if identifiers.len() != items.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "The items contain duplicate identifiers",
            ));
        }

        Ok(Self {
            items,
            block: None,
            scrollbar: None,
            style: Style::new(),
            highlight_style: Style::new(),
            highlight_symbol: "",
            node_closed_symbol: "\u{25b6} ", // Arrow to right
            node_open_symbol: "\u{25bc} ",   // Arrow down
            node_no_children_symbol: "  ",
        })
    }

    pub fn block(mut self, block: Block<'a>) -> Self {
        self.block = Some(block);
        self
    }

    /// Show the scrollbar when rendering this widget.
    ///
    /// Experimental: Can change on any release without any additional notice.
    /// Its there to test and experiment with whats possible with scrolling widgets.
    /// Also see <https://github.com/ratatui-org/ratatui/issues/174>
    pub const fn experimental_scrollbar(mut self, scrollbar: Option<Scrollbar<'a>>) -> Self {
        self.scrollbar = scrollbar;
        self
    }

    pub const fn style(mut self, style: Style) -> Self {
        self.style = style;
        self
    }

    pub const fn highlight_style(mut self, style: Style) -> Self {
        self.highlight_style = style;
        self
    }

    pub const fn highlight_symbol(mut self, highlight_symbol: &'a str) -> Self {
        self.highlight_symbol = highlight_symbol;
        self
    }

    pub const fn node_closed_symbol(mut self, symbol: &'a str) -> Self {
        self.node_closed_symbol = symbol;
        self
    }

    pub const fn node_open_symbol(mut self, symbol: &'a str) -> Self {
        self.node_open_symbol = symbol;
        self
    }

    pub const fn node_no_children_symbol(mut self, symbol: &'a str) -> Self {
        self.node_no_children_symbol = symbol;
        self
    }
}

#[test]
#[should_panic = "duplicate identifiers"]
fn tree_new_errors_with_duplicate_identifiers() {
    let item = TreeItem::new_leaf("same", "text");
    let another = item.clone();
    let items = [item, another];
    let _: Tree<_> = Tree::new(&items).unwrap();
}

impl<Identifier> StatefulWidget for Tree<'_, Identifier>
where
    Identifier: Clone + PartialEq + Eq + core::hash::Hash,
{
    type State = TreeState<Identifier>;

    #[allow(clippy::too_many_lines)]
    fn render(self, full_area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        buf.set_style(full_area, self.style);

        // Get the inner area inside a possible block, otherwise use the full area
        let area = self.block.map_or(full_area, |block| {
            let inner_area = block.inner(full_area);
            block.render(full_area, buf);
            inner_area
        });

        state.last_area = area;
        state.last_rendered_identifiers.clear();
        if area.width < 1 || area.height < 1 {
            return;
        }

        let visible = state.flatten(self.items);
        state.last_biggest_index = visible.len().saturating_sub(1);
        if visible.is_empty() {
            return;
        }
        let available_height = area.height as usize;

        let ensure_index_in_view =
            if state.ensure_selected_in_view_on_next_render && !state.selected.is_empty() {
                visible
                    .iter()
                    .position(|flattened| flattened.identifier == state.selected)
            } else {
                None
            };

        // Ensure last line is still visible
        let mut start = state.offset.min(state.last_biggest_index);

        if let Some(ensure_index_in_view) = ensure_index_in_view {
            start = start.min(ensure_index_in_view);
        }

        let mut end = start;
        let mut height = 0;
        for item_height in visible
            .iter()
            .skip(start)
            .map(|flattened| flattened.item.height())
        {
            if height + item_height > available_height {
                break;
            }
            height += item_height;
            end += 1;
        }

        if let Some(ensure_index_in_view) = ensure_index_in_view {
            while ensure_index_in_view >= end {
                height += visible[end].item.height();
                end += 1;
                while height > available_height {
                    height = height.saturating_sub(visible[start].item.height());
                    start += 1;
                }
            }
        }

        state.offset = start;
        state.ensure_selected_in_view_on_next_render = false;

        if let Some(scrollbar) = self.scrollbar {
            let mut scrollbar_state = ScrollbarState::new(visible.len().saturating_sub(height))
                .position(start)
                .viewport_content_length(height);
            let scrollbar_area = Rect {
                // Inner height to be exactly as the content
                y: area.y,
                height: area.height,
                // Outer width to stay on the right border
                x: full_area.x,
                width: full_area.width,
            };
            scrollbar.render(scrollbar_area, buf, &mut scrollbar_state);
        }

        let blank_symbol = " ".repeat(self.highlight_symbol.width());

        let mut current_height = 0;
        let has_selection = !state.selected.is_empty();
        #[allow(clippy::cast_possible_truncation)]
        for flattened in visible.iter().skip(state.offset).take(end - start) {
            let Flattened { identifier, item } = flattened;

            let x = area.x;
            let y = area.y + current_height;
            let height = item.height() as u16;
            current_height += height;

            let area = Rect {
                x,
                y,
                width: area.width,
                height,
            };

            let text = &item.text;
            let item_style = text.style;

            let is_selected = state.selected == *identifier;
            let after_highlight_symbol_x = if has_selection {
                let symbol = if is_selected {
                    self.highlight_symbol
                } else {
                    &blank_symbol
                };
                let (x, _) = buf.set_stringn(x, y, symbol, area.width as usize, item_style);
                x
            } else {
                x
            };

            let after_depth_x = {
                let indent_width = flattened.depth() * 2;
                let (after_indent_x, _) = buf.set_stringn(
                    after_highlight_symbol_x,
                    y,
                    " ".repeat(indent_width),
                    indent_width,
                    item_style,
                );
                let symbol = if item.children.is_empty() {
                    self.node_no_children_symbol
                } else if state.opened.contains(identifier) {
                    self.node_open_symbol
                } else {
                    self.node_closed_symbol
                };
                let max_width = area.width.saturating_sub(after_indent_x - x);
                let (x, _) =
                    buf.set_stringn(after_indent_x, y, symbol, max_width as usize, item_style);
                x
            };

            let text_area = Rect {
                x: after_depth_x,
                width: area.width.saturating_sub(after_depth_x - x),
                ..area
            };
            text.render(text_area, buf);

            if is_selected {
                buf.set_style(area, self.highlight_style);
            }

            state
                .last_rendered_identifiers
                .push((area.y, identifier.clone()));
        }
        state.last_identifiers = visible
            .into_iter()
            .map(|flattened| flattened.identifier)
            .collect();
    }
}

impl<Identifier> Widget for Tree<'_, Identifier>
where
    Identifier: Clone + Eq + core::hash::Hash,
{
    fn render(self, area: Rect, buf: &mut Buffer) {
        let mut state = TreeState::default();
        StatefulWidget::render(self, area, buf, &mut state);
    }
}

#[cfg(test)]
mod render_tests {
    use super::*;

    #[must_use]
    #[track_caller]
    fn render(width: u16, height: u16, state: &mut TreeState<&'static str>) -> Buffer {
        let items = TreeItem::example();
        let tree = Tree::new(&items).unwrap();
        let area = Rect::new(0, 0, width, height);
        let mut buffer = Buffer::empty(area);
        StatefulWidget::render(tree, area, &mut buffer, state);
        buffer
    }

    #[test]
    fn does_not_panic() {
        _ = render(0, 0, &mut TreeState::default());
        _ = render(10, 0, &mut TreeState::default());
        _ = render(0, 10, &mut TreeState::default());
        _ = render(10, 10, &mut TreeState::default());
    }

    #[test]
    fn nothing_open() {
        let buffer = render(10, 4, &mut TreeState::default());
        #[rustfmt::skip]
        let expected = Buffer::with_lines([
            "  Alfa    ",
            "▶ Bravo   ",
            "  Hotel   ",
            "          ",
        ]);
        assert_eq!(buffer, expected);
    }

    #[test]
    fn depth_one() {
        let mut state = TreeState::default();
        state.open(vec!["b"]);
        let buffer = render(13, 7, &mut state);
        let expected = Buffer::with_lines([
            "  Alfa       ",
            "▼ Bravo      ",
            "    Charlie  ",
            "  ▶ Delta    ",
            "    Golf     ",
            "  Hotel      ",
            "             ",
        ]);
        assert_eq!(buffer, expected);
    }

    #[test]
    fn depth_two() {
        let mut state = TreeState::default();
        state.open(vec!["b"]);
        state.open(vec!["b", "d"]);
        let buffer = render(15, 9, &mut state);
        let expected = Buffer::with_lines([
            "  Alfa         ",
            "▼ Bravo        ",
            "    Charlie    ",
            "  ▼ Delta      ",
            "      Echo     ",
            "      Foxtrot  ",
            "    Golf       ",
            "  Hotel        ",
            "               ",
        ]);
        assert_eq!(buffer, expected);
    }
}

