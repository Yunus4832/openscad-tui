# OpenSCAD TUI - Terminal User Interface

一个基于 Rust 实现的 OpenSCAD 命令行用户界面，支持类似 Vim 的交互方式，用于快速生成 OpenSCAD 代码。

## 项目结构

```
openscad-tui/
├── crates/
│   ├── core/                    # 核心 AST 模块
│   │   └── src/
│   │       └── lib.rs          # AST 定义和操作 API
│   ├── library/                 # 库管理模块
│   │   └── src/
│   │       └── lib.rs          # 库加载和模块发现
│   └── ui/                      # UI 交互模块
│       └── src/
│           ├── main.rs         # 应用入口
│           ├── lib.rs          # 库导出
│           ├── app.rs          # 应用状态
│           ├── ui.rs           # UI 渲染
│           ├── input.rs        # 输入处理
│           └── commands.rs     # 命令实现
├── Cargo.toml                   # 工作空间配置
└── openscad.md                  # 需求文档
```

## 功能模块说明

### 1. 核心模块 (openscad-core)

提供 OpenSCAD AST 的完整实现：

- **AST 数据结构**: `Expr`, `ModuleNode`, `AstRoot`
- **表达式类型**: 支持布尔值、数字、字符串、列表、范围、操作符等
- **模块节点**: 支持参数、子模块、显示名称
- **序列化**: Serde 支持 YAML 序列化

关键 API:
```rust
// 创建 AST
let mut ast = AstRoot::new();

// 添加模块
ast.add_module(ModuleNode::new_leaf(
    "cube1".to_string(),
    "cube".to_string(),
    vec![Argument::Named {
        name: "size".to_string(),
        value: Expr::List(vec![Expr::Integer(10), ...]),
    }],
))?;

// 查找和删除节点
ast.delete_node("cube1")?;

// 生成代码
let code = ast.to_scad();
```

### 2. 库管理模块 (openscad-library)

提供内建模块和库加载功能：

- **内建模块**: cube, sphere, cylinder, square, circle, translate, rotate, scale, 等
- **库加载**: 从 YAML 文件加载第三方库
- **模块发现**: 获取所有可用模块及其参数

关键 API:
```rust
let mut manager = LibraryManager::new();

// 获取模块定义
let cube_def = manager.get_module("cube")?;

// 加载外部库
manager.load_library(Path::new("my_library.yaml"))?;

// 获取所有模块
let modules = manager.get_all_modules();
```

### 3. UI 交互模块 (openscad-ui)

提供命令行交互界面和命令处理：

- **UI 布局**: 三层界面 - 树形视图、命令输入、代码预览
- **Vim 风格交互**: jkhl 导航、i/a 插入、v 选择、d 删除、u 撤销等
- **命令处理**: 支持多种命令 (insert, delete, union, difference 等)
- **状态管理**: 撤销/重做栈、选择管理、错误处理

关键命令：
```
i         - 下方插入模块
a         - 上方插入模块
v         - 选择/取消选择当前节点
dd/D      - 删除当前节点
u         - 撤销
:         - 进入命令模式
:insert <module_name> [params]  - 插入模块
:union    - 合并选中节点
:difference - 差集
:write <filename> - 保存为 YAML
:load <filename>  - 加载 YAML
:export <filename> - 导出为 .scad
```

## 编译和运行

### 编译

```bash
cd /home/yunus/Desktop/temp/rust-test/openscad-tui
cargo build --release
```

### 运行

```bash
cargo run --bin openscad-tui
```

### 测试

```bash
cargo test
```

## 使用示例

1. **创建基本模型**

```
i cube [10,10,10]      # 插入一个 10x10x10 的立方体
i sphere 5             # 插入一个半径为 5 的球体
```

2. **应用变换**

```
:translate [5, 0, 0]   # 移动
:rotate [45, 45, 0]    # 旋转
```

3. **布尔操作**

选择两个节点 (v 命令)，然后：
```
:difference            # 执行差集操作
:union                 # 执行并集
```

4. **保存和导出**

```
:write project.yaml    # 保存项目为 YAML (便于再次编辑)
:export model.scad     # 导出为 OpenSCAD 代码
```

## 库定义格式

库定义文件采用 YAML 格式，例如：

```yaml
name: MyLibrary
description: My custom OpenSCAD library
file: my_library.scad
version: 1.0
modules:
  - name: custom_cube
    description: A customized cube
    accepts_children: false
    parameters:
      - name: size
        param_type: list
        default: "[10, 10, 10]"
        description: "Cube dimensions [x, y, z]"
```

## 架构设计要点

1. **三层架构**:
   - Core: 数据结构和基本操作
   - Library: 业务逻辑 (库管理)
   - UI: 用户交互和 TUI 实现

2. **错误处理**: 统一使用 `thiserror` 库

3. **序列化**: 使用 `serde_yaml` 支持项目保存/加载

4. **TUI**: 使用 `ratatui` 和 `crossterm` 提供现代化终端界面

## 后续扩展方向

1. **高级编辑功能**:
   - 复制粘贴 (y/p 命令)
   - 节点拖拽重排序
   - 参数实时编辑

2. **性能优化**:
   - 大型 AST 的增量渲染
   - 代码生成缓存

3. **集成**:
   - OpenSCAD 渲染预览
   - 实时编译反馈

4. **高级操作**:
   - 参数化模块
   - 条件表达式支持
   - 自定义模块定义

## 许可证

MIT
