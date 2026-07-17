# OpenSCAD TUI

OpenSCAD TUI 是一个使用 Rust、Ratatui 和 Crossterm 编写的终端结构化编辑器。它通过
Vim 风格按键和命令修改 OpenSCAD AST，在终端中展示模型树与生成的 `.scad` 源码。

> 当前项目处于可运行的原型阶段。右侧预览是 OpenSCAD 源码预览，不是 2D/3D 几何渲染；
> 程序目前不会调用 OpenSCAD 编译器。

## 当前能力

- 在树形界面中创建、选择、嵌套和删除 OpenSCAD 模块
- 对选中节点应用 `translate`、`rotate`、`scale`
- 使用 `union`、`difference`、`intersection` 组合选中节点
- 定义全局变量、自定义函数和自定义模块
- 生成并导出 OpenSCAD 源码
- 将可编辑项目保存为 JSON，并从 JSON 恢复
- 从 JSON 加载模块和函数元数据作为补全库
- 撤销、重做和命令历史
- 复制、粘贴、移除和替换模块节点
- 补全命令、模块、参数、值、函数和文件路径
- 在函数调用与列表嵌套表达式中继续补全

核心表达式支持布尔值、整数、浮点数、字符串、`undef`、标识符、列表、范围、
一元/二元运算、三元表达式、索引和函数调用。

## 构建与运行

需要 Rust 2021 edition 兼容工具链。

```bash
cargo build --workspace
cargo run --bin openscad-tui
```

发布构建：

```bash
cargo build --release
./target/release/openscad-tui
```

## 基本交互

普通模式下可使用：

| 按键 | 功能 |
| --- | --- |
| `j` / `k`、`↓` / `↑` | 移动树光标 |
| `h` / `l`、`←` / `→` | 折叠或展开节点 |
| `Enter` | 切换节点展开状态 |
| `v` | 选择或取消选择当前节点 |
| `y` / `p` | 复制最后选中的节点子树（否则复制当前节点）／在当前位置后粘贴 |
| `x` | 移除所有选中节点并提升其子节点；无选中时操作当前节点 |
| `c` | 进入 `replace`，替换所有选中节点；无选中时操作当前节点 |
| `a` / `A` | 进入 `set` / `unset`，设置或移除选中节点或当前节点的显式参数 |
| `i` | 打开 `insert` 命令 |
| `t` / `r` / `s` | 打开平移、旋转或缩放命令 |
| `d` | 删除所有选中节点的完整子树；无选中时删除当前节点、global、function 或 module 定义 |
| `u` / `Ctrl+R` | 撤销或重做 |
| `w` / `e` | 保存或加载 JSON 项目 |
| `L` | 加载 JSON 库 |
| `:` | 进入命令模式 |
| `?` | 显示帮助 |
| `q` / `Ctrl+C` | 退出 |

命令输入模式支持方向键、Home、End、Backspace、Delete、命令历史，以及 `Tab` 补全。
存在多个候选时重复按 `Tab` 可切换候选，按 `Enter` 应用当前候选。

结构编辑命令统一遵循“选中节点优先，否则使用当前节点”。`delete`
删除整个子树；`remove` 只删除目标节点并将子节点提升到父节点；`replace`
删除目标子树后在原位置插入新节点。

## 常用命令

命令模式由 `:` 进入。命令文本本身不需要带冒号。

### 插入模块

```text
insert cube size=[10, 10, 10], center=true
i sphere r=5, $fn=64
```

如果省略参数，程序会进入第二阶段参数输入：

```text
insert cylinder
```

容器模块需要先使用 `v` 选择子节点：

```text
translate v=[10, 0, 0]
rotate a=[0, 0, 45]
scale v=[2, 1, 1]
union
difference
intersection
```

### 定义全局变量

```text
global size=10
global label="demo"
global $fn=64
```

`global` 的值补全保持精简，只提供已定义的全局变量以及 `true`、`false`、
`undef`。较复杂的计算建议定义为 function。
再次定义同名 global 会原位覆盖旧值。选中 global 后按 `d` 可删除定义；已有引用不会级联删除。

### 定义函数

```text
function square(x) = x * x
function add(a, b) = a + b
function distance2d(x, y) = sqrt(x * x + y * y)
function pi_value() = 3.14159
```

函数名必须是合法标识符。当前函数参数不支持默认值。
在 `=` 后按 `Tab` 可补全当前函数参数、全局变量、布尔常量以及内置或自定义函数；
函数候选确认后会自动追加 `(`。
定义成功后，函数会出现在参数值补全候选中。
再次定义同名 function 会原位覆盖旧定义。选中 function 后按 `d` 可删除定义；已有调用不会级联删除。

### 定义模块

`module` 会复制当前选中的节点作为模块定义的主体：

```text
module post radius=2, height=20
module holder size=[10, 20, 5]
```

模块参数可以有默认值，也可以只写参数名：

```text
module example width=10, height, center=false
```

定义成功后，自定义模块会进入模块库和补全候选。
再次定义同名 module 会原位覆盖旧定义。选中 module 定义后按 `d` 可删除定义；已有实例不会级联删除。

### 修改节点参数

`set` 会优先修改选中节点，没有选中节点时修改当前节点：

```text
set size=20
set center=true
set v=offset
```

参数值可以引用当前 module 定义的参数。例如在 `my_box` 的主体节点上执行：

```text
set size=size
set center=center
```

会生成类似：

```scad
module my_box(size=10, center=false) {
    cube(size=size, center=center);
}
```

`set` 的参数名补全来自目标节点的模块定义，值补全会包含当前 module
作用域参数、全局变量、布尔常量和函数。

`unset` 会移除显式传入的参数，使模块重新使用其默认值：

```text
unset size
unset center
```

它同样优先操作选中节点，没有选中节点时操作当前节点。参数未被显式设置时会报错。

### 文件操作

```text
write project.json
write! project.json
edit project.json
edit! project.json
export model.scad
library my_library.json
wq
```

- `write` / `edit` 保存和读取可继续编辑的 JSON AST。
- 带 `!` 的版本允许覆盖未保存状态相关的保护。
- `export` 只生成 `.scad` 文件，不会运行 OpenSCAD。
- `library` 加载的是补全及模块元数据；对应的 OpenSCAD 库文件需要在实际使用环境中可用。

## JSON 库格式

库文件需要包含 `modules` 和 `functions` 数组。最小示例：

```json
{
  "name": "ExampleLibrary",
  "description": "Example OpenSCAD metadata",
  "file": "example.scad",
  "version": "1.0",
  "modules": [
    {
      "name": "rounded_cube",
      "description": "Cube with rounded edges",
      "accepts_children": false,
      "parameters": [
        {
          "name": "size",
          "param_type": "list",
          "default": "[10, 10, 10]",
          "description": "Cube dimensions"
        }
      ]
    }
  ],
  "functions": [
    {
      "name": "double",
      "description": "Double a value",
      "parameters": [
        {
          "name": "x",
          "param_type": "number",
          "description": "Input value"
        }
      ],
      "return_type": "number"
    }
  ]
}
```

参数的 `default` 和 `description` 是可选字段。可参考仓库中的 `stdlib.json` 查看更多定义。

## 完整命令概览

- 导航：`next`、`prev`、`collapse`、`expand`、`toggle`
- 选择：`select`、`deselect-all`
- 编辑：`insert`、`delete`、`undo`、`redo`
- 节点操作：`yank`、`paste`、`remove`、`replace`
- 变换：`translate`、`rotate`、`scale`
- 布尔操作：`union`、`difference`、`intersection`
- 定义：`global`、`function`、`module`
- 文件：`write`、`write!`、`edit`、`edit!`、`export`、`library`
- 系统：`help`、`quit`、`quit!`、`wq`

节点操作示例：

```text
yank
paste
remove
replace sphere
```

- `yank` 将当前节点的整个子树复制到应用内剪贴板。
- `paste` 在当前节点后粘贴，并为整个子树生成新 ID；选中 Modules 或模块定义时追加到其主体。
- `remove` 只删除节点本身，将其直接子节点按原顺序提升到父节点的原位置，并且不改变剪贴板。
- `replace <module_name> [params]` 替换当前节点。它与 `insert` 使用相同的模块名、参数名和参数值多阶段补全；省略参数时会进入第二阶段参数输入。确认参数后，当前节点整棵子树才会被删除，并在相同位置插入具有新 ID 的新模块；取消输入不会修改 AST。

## 项目结构

```text
openscad-tui/
├── crates/
│   ├── core/       # AST、表达式解析和 OpenSCAD 代码生成
│   ├── library/    # 内置/外部模块与函数元数据
│   └── ui/         # TUI、输入、命令和应用状态
├── stdlib.json     # 内置模块与函数元数据
└── Cargo.toml      # Cargo workspace
```

## 开发与验证

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo check --workspace
```

当前工作区包含 114 个独立单元测试：core 30 个、library 9 个、ui 75 个。

## 当前限制

- 没有 OpenSCAD 编译、几何渲染或编译错误反馈
- 不能导入和解析已有 `.scad` 源文件
- 不是自由文本源码编辑器，主要通过 AST 树和命令编辑
- 函数参数默认值、语义类型检查和高级补全仍有限
- 鼠标事件目前不用于树节点操作
- README 描述的是当前代码状态，项目尚未提供稳定发布或安装包

## License

MIT
