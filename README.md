# OpenSCAD TUI

OpenSCAD TUI 是一个使用 Rust、Ratatui 和 Crossterm 编写的终端结构化编辑器。它通过
Vim 风格按键和命令修改 OpenSCAD AST，可在终端中查看模型树、生成的 `.scad` 源码，
也可调用本机 OpenSCAD 生成网格并显示交互式模型预览。

> 当前项目处于可运行的原型阶段，未发布版本号为 `0.0.1`，尚未提供稳定发布或安装包。

## 当前能力

- 在树形界面中创建、选择、嵌套和删除 OpenSCAD 模块
- 对选中节点应用 `translate`、`rotate`、`scale`
- 使用 `union`、`difference`、`intersection` 组合选中节点
- 定义全局变量、自定义函数和自定义模块
- 生成并导出 OpenSCAD 源码
- 将可编辑项目保存为版本化 `.scadtui` 包，并从项目包恢复
- 将多文件 `.scad` 项目解析为 globals、functions、module definitions 和 module nodes，并嵌入项目包
- 在同一实例中切换、编辑多个项目源文件，当前 buffer 同时也是默认渲染入口
- 直接加载 `.scad` 库并提取模块、函数定义用于补全
- 撤销、重做和命令历史
- 复制、粘贴、移除和替换模块节点
- 补全命令、模块、参数、值、函数和文件路径
- 在函数调用与列表嵌套表达式中继续补全
- 调用 OpenSCAD 生成 OFF 网格，并通过 CPU 光栅化显示交互式模型预览
- 将多个可编辑项目源编译为共享 mesh，在独立 Assembly Screen 中分层装配、变换和预览
- 将装配以保留零件层级与几何实例的白膜 COLLADA `.dae` 场景导出
- 切换透视/正交投影、标准视角、相机环绕/平移/缩放和自动旋转

核心表达式支持布尔值、整数、浮点数、字符串、`undef`、标识符、列表、范围、
一元/二元运算、三元表达式、索引和函数调用。

## 构建与运行

需要 Rust 2021 edition 兼容工具链。源码编辑、项目读写和 `.scad` 导出不依赖
OpenSCAD；使用 `render` 或首次进入模型预览时，还需要 PATH 中存在 `openscad`
可执行文件。OpenSCAD 调用的超时时间为 120 秒。

```bash
cargo build --workspace
cargo run --bin openscad-tui
cargo run --bin openscad-tui -- existing.scad
cargo run --bin openscad-tui -- --version
```

也可以直接打开项目包：`openscad-tui project.scadtui`。直接传入 `.scad` 与执行
`edit existing.scad` 的导入行为相同。
版本号统一来自 workspace package；CLI 使用 `--version` / `-V`，TUI 命令模式使用
`version`（别名 `ver`）查询。

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
| `Enter` | 切换节点展开状态；在 `[Project Sources]` 中打开可编辑源文件 |
| `v` | 选择或取消选择当前节点 |
| `Space` | 显示或隐藏当前节点（存在多选时作用于所有选中节点） |
| `y` / `p` | 复制最后选中的节点子树（否则复制当前节点）／在当前位置后粘贴剪贴板中的子树 |
| `x` | 移除所有选中节点并提升其子节点；无选中时操作当前节点 |
| `c` | 进入 `replace`，替换所有选中节点；无选中时操作当前节点 |
| `a` / `A` | 进入 `set` / `unset`，设置或移除选中节点或当前节点的显式参数 |
| `i` / `I` | 打开 `insert` / `insert-before`，在当前节点后／前插入 |
| `t` / `r` / `s` | 打开平移、旋转或缩放命令 |
| `d` | 剪切所有选中节点的完整子树；定义项仍直接删除；光标移动到后一个兄弟节点，末项则移动到前一个 |
| `u` / `Ctrl+R` | 撤销或重做 |
| `P` / `R` | 切换源码/模型预览；强制重新渲染当前 buffer 并进入模型预览 |
| `w` | 保存 `.scadtui` 项目包 |
| `o` | 打开 `open` 命令，加载 `.scadtui` 项目包 |
| `e` | 打开 `edit` 命令，解析 `.scad` 文件进行结构化编辑 |
| `L` | 加载并嵌入 `.scad` 源码库 |
| `:` | 进入命令模式 |
| `?` | 显示帮助 |
| `q` / `Ctrl+C` | 退出 |

命令输入模式支持方向键、Home、End、Backspace、Delete、命令历史，以及 `Tab` 补全。
存在多个候选时重复按 `Tab` 可切换候选，按 `Enter` 应用当前候选。
最近 100 条用户命令会跨启动持久化到平台本地数据目录的
`openscad-tui/history.json`（Linux 通常为 `~/.local/share/openscad-tui/history.json`）。

结构编辑命令统一遵循“选中节点优先，否则使用当前节点”。`delete`
剪切整个模块子树到应用内剪贴板；`remove` 只删除目标节点并将子节点提升到父节点；`replace`
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
insert-before sphere r=5
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

内建签名表也包含 2D 多边形和两种拉伸模块，因此它们可以直接通过命令创建并参与参数补全：

```text
insert polygon points=[[0,0], [20,0], [10,15]], paths=[[0,1,2]]
linear_extrude height=8, twist=30, slices=20
rotate_extrude angle=270, convexity=4
```

`linear_extrude` 和 `rotate_extrude` 是容器模块，需要先用 `v` 选择一个或多个 2D 子节点。
单独的 `polygon`、`square` 或 `circle` 是二维对象，OpenSCAD 不能把它们直接导出为
三维 OFF 网格；渲染前需要先使用其中一种拉伸模块。

`stdlib.json` 通过 `include_str!` 编译进程序，不是运行时配置文件。它只描述命令补全和
参数提示所需的常用内建签名，并不等同于 OpenSCAD 的完整标准库；当前目录仍有未覆盖
的内建模块，外部 SCAD 库则通过 `library` 加载。

### 定义全局变量

```text
global size=10
global label="demo"
global $fn=64
global points=[[0, 0], [10, 20], [sin(angle), height]]
```

输入 `global <前缀><Tab>` 可补全已有全局变量名并自动追加 `=`，方便原位重定义。
值表达式可补全已定义的全局变量、内置或自定义函数以及 `true`、`false`、`undef`，
并支持列表和任意层级的嵌套列表。
再次定义同名 global 会原位覆盖旧值。选中 global 后按 `d` 可删除定义；已有引用不会级联删除。

### 定义函数

```text
function square(x) = x * x
function add(a, b) = a + b
function distance2d(x, y) = sqrt(x * x + y * y)
function pi_value() = 3.14159
```

函数名必须是合法标识符。当前函数参数不支持默认值。
输入 `function <前缀><Tab>` 可补全已有函数名并自动追加 `(`。
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

`set` 的参数名补全来自目标节点的模块定义。目标参数已有显式值时，候选会直接回显
完整赋值（例如 `size=[10, 20, 5]`），确认后可从值末尾继续修改；多选节点的值不一致时
只补全 `size=`，避免误用其中某个节点的值。值补全还会包含当前 module 作用域参数、
全局变量、布尔常量和函数。

`unset` 会移除显式传入的参数，使模块重新使用其默认值：

```text
unset size
unset center
```

它同样优先操作选中节点，没有选中节点时操作当前节点。参数未被显式设置时会报错。

### 文件操作

```text
write project.scadtui
write! project.scadtui
open project.scadtui
open! project.scadtui
edit existing.scad
new project
new file part.scad
export source model.scad
export tree ./source-tree
export model model.stl
export model model.dae
view model.off
view model.stl
library gears.scad
use gears.scad
include gears.scad
buffer
buffer vernier_cursor.scad
buffer next
render
assembly new robot
assembly add vernier_body.scad body
assembly add vernier_cursor.scad cursor
assembly translate cursor 20 0 0
assembly render
assembly export vernier.dae
wq
```

- `write` 保存可继续编辑的 `.scadtui` 项目包，`open` 读取项目包。项目包是 ZIP 容器，
  包含版本化 `manifest.json`、结构化项目数据和可检查的 `sources/` SCAD 快照。
- `new project` 创建包含空白 `main.scad` 的项目，`new file part.scad` 在当前项目中新建
  可编辑 source；`new! project` 可以明确丢弃未保存修改。
- `edit` 将已有 `.scad` 文件及其本地依赖导入当前项目，连续执行会归集为多个可编辑
  buffer，不会替换已有 source。导入后内容与原文件解耦。
- `edit` 会递归收集能从项目目录或 OpenSCAD 库目录解析到的 `include` / `use` 文件，将
  完整源码 AST、定义索引和依赖类型一起嵌入项目包。项目目录内的主文件与配件文件可编辑；
  BOSL 等外部库保持只读，但仍会参与补全和渲染。
- `[Project Sources]` 只展示项目内可编辑的 `entry` 和 `part` 文件，已加载或经
  `use` / `include` 引入的只读库不会显示在这里。`*` 表示当前编辑缓冲区；在源文件
  上按 `Enter` 即可切换。
- `buffer [source|next|prev]` 列出或切换可编辑源文件，文件名、文件 stem 和项目内相对路径
  都可使用。切换前会把当前 AST 写回其嵌入记录，切换后模块和函数补全按新文件的可达依赖重建。
- `render` 直接渲染当前编辑文件。渲染时所有项目文件都会从各自 AST 生成到临时目录，
  外部只读库则恢复导入时的原文，再由 OpenSCAD 按原有 `include` / `use` 关系处理。
- `edit` 后需使用 `write project.scadtui` 保存项目；不会覆盖原始 `.scad` 文件。
- `open!` 和 `new! project` 允许明确丢弃未保存状态；`edit` 是增量操作，不需要 `edit!`。
- `export source <file.scad>` 只导出当前 buffer；`export tree <directory>` 导出当前 buffer
  及其可达依赖组成的完整 SCAD 源码树；`export model <artifact>` 按目标后缀生成模型产物。
  `.stl`、`.3mf` 等格式仍由 OpenSCAD 原生导出；`.dae` 会先生成统一三角网格，再输出
  静态 COLLADA 1.4.1 几何。DAE 导出不包含骨骼、动画、材质、相机或通用场景语义。
  相对导出路径以当前 `.scadtui` 项目包所在目录为基准；没有项目文件时才使用启动程序时
  的工作目录。
- `assembly` 管理与源码 AST 分离的刚性零件装配。`assembly add` 只接受项目中的可编辑
  source，每个唯一 source 由 OpenSCAD 编译一次并在实例之间共享 mesh；平移、旋转、缩放、
  pivot、父子关系和可见性变化只重新组装/光栅化场景，不会再次调用 OpenSCAD。完整命令见下节。
- `view <model.off|model.stl>` 直接加载现有 OFF 或 ASCII/Binary STL 文件并进入模型预览，
  不修改当前项目，也不调用 OpenSCAD。也可以把 `.off` / `.stl` 文件作为启动参数。
- `library gears.scad` 加载 OpenSCAD 源码库并递归收集本地 SCAD 依赖，但不会修改
  当前 source 的语义。源码会直接嵌入项目包，不需要额外的库描述文件。
- `use <source>` / `include <source>` 在当前 buffer 与项目内另一个 source 之间建立对应
  关系；目标既可以是 `library` 加载的只读库，也可以是 `edit` 或 `new file` 创建的可编辑
  配件。`use` 只导入模块和函数定义，`include` 还保留顶层变量和建模语句语义。
- 只有建立 `use` 或 `include` 关系后，目标及其可达依赖中的定义才会进入当前 buffer 的
  补全并参与渲染。已加载但未引用的 source 仍会随项目保存，之后可随时使用。

## 完整命令概览

- 导航：`next`、`prev`、`collapse`、`expand`、`toggle`
- 选择：`select`、`deselect-all`
- 编辑：`insert`、`insert-before`、`delete`、`undo`、`redo`
- 节点操作：`yank`、`paste`、`remove`、`replace`、`visibility show|hide|toggle`
- 变换：`translate`、`rotate`、`scale`
- 布尔操作：`union`、`difference`、`intersection`
- 定义：`global`、`function`、`module`
- 文件：`new`、`new!`、`write`、`write!`、`open`、`open!`、`edit`、`buffer`、`export`、`library`、`use`、`include`
- 预览：`render`、`view`、`preview source|model|toggle|close`、`camera ...`、`axes ...`、`protocol ...`
- 装配：`assembly new|open|list|add|select|copy|paste|remove|parent|translate|rotate|scale|pivot|visibility|render|export|close`
- 系统：`help`、`version`、`diagnostics [file]`、`quit`、`quit!`、`wq`

## 零件装配与 DAE 导出

装配是 `.scadtui` 项目中的独立数据模型。源码 buffer 只负责产生不可变 mesh；装配保存
mesh 来源、零件实例、父子层级、局部 TRS、pivot 和可见性。它不会把变换写回 OpenSCAD AST。

```text
assembly new robot
assembly add body.scad body
assembly add parts/arm.scad left_arm
assembly add parts/arm.scad right_arm
assembly parent left_arm body
assembly parent right_arm body
assembly translate left_arm -12 0 8
assembly translate right_arm 12 0 8
assembly rotate right_arm 0 0 180
assembly scale body 1 1 1.2
assembly pivot left_arm 0 0 4
assembly visibility right_arm toggle
assembly copy right_arm
assembly paste root
assembly render
assembly export exports/robot.dae
```

`assembly open [name]` 切换装配，`assembly list` 列出项目中的装配，`assembly select`、
`assembly remove` 和 `assembly parent <part> root` 分别选择、移除和解除父级。命令参数支持
项目 source 与零件 ID 的补全。相对导出路径仍以 `.scadtui` 所在目录为基准；省略 `.dae`
后缀会自动补齐。`parent`、四种变换和 `visibility` 都可以省略零件 ID，默认作用于当前
选中的零件；显式 ID 形式继续保留，便于脚本和 CLI 使用。重复添加或粘贴同名零件时，
显示名与 ID 会依次变为 `arm`、`arm2`、`arm3`。`assembly copy [part]` 复制一个零件实例，
`assembly paste [parent|root]` 保留其 source、变换、可见性以及默认父级，并产生新的唯一名字。

`pivot` 是零件局部坐标中的旋转/缩放中心，不是额外位移。实际变换顺序为
`translate × pivot × rotate × scale × -pivot`。例如把 pivot 设为 `[10, 0, 0]` 后绕 Z 轴
旋转，零件会绕局部坐标中的 `[10, 0, 0]` 转动，而不是绕原点转动；制作门轴、车轮轴、
机械臂关节时很有用。普通摆放只需要 translate/rotate/scale，可以一直保持 pivot 为零。

Assembly Screen 左侧显示零件层级与当前零件的 source、父级、可见性和 TRS/pivot，右侧
显示共享多网格场景。`j/k` 选择零件，`v` 切换其可见性，`Space` 与 Model Screen 一致用于
启停自动旋转，`d` 删除，`x` 切换坐标轴，`R` 重新编译并渲染，`Esc/q/P` 返回 Source。
`a`、`n`、`e` 分别预填 add、new、export 命令；`t/r/s/o/g` 会把当前零件及其
translate/rotate/scale/pivot/parent 值带入命令行，直接修改即可。鼠标可选择零件并使用与
Model Screen 相同的环绕、右键平移和滚轮缩放；`y/p` 复制和粘贴零件。Assembly 中的
`p` 因此不再切换投影，投影仍可点击工具栏按钮或执行 `camera projection toggle`。所有按键和按钮都通过已注册的
`assembly`、`camera`、`axes`、`protocol` 命令执行。

装配导出的 COLLADA 1.4.1 是刻意受限的白膜交换格式：包含具名且唯一的零件节点、
去重后的三角几何、法线、局部矩阵和 geometry instances。装配不会额外注入根节点；所有
未设置 parent 的零件会直接成为 visual scene 的顶层节点，根层级完全由装配数据决定。相同
source 的多个零件节点会共享一份 geometry，这是 DAE 的实例化语义；零件名属于 node，
geometry 名代表共享 mesh 数据。不包含材质/贴图、UV、动画、骨骼、蒙皮、
灯光或相机。需要这些内容时，应把 DAE 继续交给 Blender 或游戏资产管线处理。

## 模型预览

执行 `render` 会调用本机 `openscad`，把当前模型编译为内部三角网格；`view` 则直接将
OFF/STL 文件加载为相同的 `Mesh`。单模型会适配为一个 instance；装配则提供多个共享 mesh
和实例矩阵。三条路径共用 `openscad-render` 的 `RenderScene` 后台 CPU 光栅化，
输出 RGBA 帧。OFF 是 OpenSCAD 编译路径的内部中间格式，不会泄漏到下游渲染接口。
终端展示由独立的 `openscad-terminal` 后端处理，不参与模型加载、相机计算和光栅化。

终端后端支持 Kitty、Sixel、iTerm2、Halfblocks、Braille 和 ASCII。编码工作线程始终只
保留一个正在处理的请求和一个可覆盖的最新请求；新帧编码完成前继续显示已有前帧，
避免连续预览中的空白和闪烁。Kitty 使用 RGB24、快速 zlib 和稳定 image ID，Sixel
使用无 diffusion 的快速编码。Halfblocks 直接把两行 RGBA 像素映射为一个终端
单元格；Braille 使用每格 2×4 个 Unicode 点表现高分辨率轮廓，并通过 Bayer 有序
抖动把增强后的面明暗映射到一至八个实心点。ASCII 根据探测到的字体单元宽高比计算相机视口，使用轻量 2×
超采样和经典长密度字符序列。两个文本后端共享背景分离、5%–95% 稳健动态范围拉伸
和保色对比度增强，避免模型被非正方形终端单元拉伸并保持 CAD 轮廓清晰；它们都不
经过通用图片缩放或 Chafa。
iTerm2 由于每帧必须传输完整内联图片，使用半线性分辨率的低延迟 JPEG，由终端
缩放到目标单元格区域，以降低 Base64 传输量和终端解码时间。
Braille 后端要求终端字体包含 Unicode Braille Patterns 字形；若字体缺失或点阵字形
宽度异常，应改用 `ascii` 或 `halfblocks`。

默认自动探测图像协议，也可以通过环境变量强制选择协议，方便比较终端中的实际
表现：

```bash
OPENSCAD_TUI_IMAGE_PROTOCOL=sixel ./target/release/openscad-tui
```

可选值为 `auto`、`kitty`、`sixel`、`iterm2`、`halfblocks`、`braille` 和 `ascii`。运行期间
也可以通过 `protocol` 命令切换后端；缓存的 RGBA 帧会立即提交给新后端，无需重新
调用 OpenSCAD。强制选择终端不支持的图像协议可能导致预览空白或显示转义字符。

```text
render
view model.off
view model.stl
preview source|model|toggle|close
camera projection perspective|orthographic|toggle
camera view front|back|left|right|top|bottom|iso
camera orbit <yaw-deg> <pitch-deg>
camera pan <x> <y>
camera zoom <factor>
camera fit
camera auto-rotate on|off|toggle
axes on|off|toggle
visibility show|hide|toggle
protocol auto|next|kitty|sixel|iterm2|halfblocks|braille|ascii
diagnostics [file]
```

Model Screen 的键盘映射、鼠标手势和底部按钮都是这些 `preview`、`camera` 命令的
快捷入口，实际行为统一由命令处理函数执行。
模型预览默认显示深度感知的世界坐标轴：正向 X/Y/Z 分别使用红、绿、蓝色，负向使用
对应的暗色。`axes` 只会用缓存网格重新光栅化，不会再次调用 OpenSCAD；所有终端协议
共享同一张包含坐标轴的 RGBA 帧。
`visibility` 对选中节点（未多选时为当前节点）设置 OpenSCAD 原生的 `*` 禁用修饰符，
因此隐藏状态属于 AST，会参与撤销、项目保存和源码导出，而不是仅在预览中临时过滤。
渲染失败时标题显示简短原因；`diagnostics` 打开完整的 OpenSCAD/显示后端诊断，
`diagnostics <file>` 将其写入文本文件，便于复制和反馈。
`preview model` 在尚无模型预览时会自动执行一次渲染；已有预览时只切换 Screen，
不会强制重新运行 OpenSCAD。

进入模型预览时会切换到独立的 Model Preview Screen：`h/j/k/l` 环绕，方向键平移，`+/-` 缩放，
`R` 强制重新渲染当前 buffer，`f` 适配模型，`p` 切换投影，`x` 切换坐标轴，
`1..7` 选择标准视角，空格切换自动旋转，
`Esc/q` 关闭模型预览。项目内预览会返回 Source；以 `.off` / `.stl` 启动的独立模型查看
会直接退出程序。
在 Editor 中按 `P` 打开模型预览；Model Preview 中按 `P` 的关闭语义与 `Esc/q` 相同。
按 `R` 会执行 `render` 命令，忽略已有预览缓存，重新渲染当前 buffer 并进入模型预览。
Model Preview Screen 同样可以按 `:` 进入 Command Mode，支持命令历史、Tab 补全和
直接执行 `camera`、`preview` 等命令。

鼠标操作会根据所在面板分发：节点树中单击定位（再次点击展开/折叠）、Ctrl+单击
切换多选、滚轮滚动；源码预览中滚轮滚动代码；模型预览中左键拖动旋转、
Shift+左键拖动或右键拖动平移、滚轮缩放。

模型预览使用全宽布局并隐藏节点树。底部工具栏首项会根据启动方式显示 Source 或 Quit，
其余按钮可以用鼠标执行 Fit、
投影切换、坐标轴、Front、Top、Iso 和自动旋转等常用操作。

节点操作示例：

```text
yank
paste
remove
replace sphere
```

- `yank` 将当前节点的整个子树复制到应用内剪贴板；`delete` 会把被删除的模块子树剪切到同一剪贴板。
- `paste` 在当前节点后按原顺序粘贴一个或多个子树，并为每棵子树生成新 ID；选中 Modules 或模块定义时追加到其主体。执行 `d` 后光标会停在最近兄弟节点，因此 `dp` 可以交换相邻节点。
- `remove` 只删除节点本身，将其直接子节点按原顺序提升到父节点的原位置，并且不改变剪贴板。
- `replace <module_name> [params]` 替换当前节点。它与 `insert` 使用相同的模块名、参数名和参数值多阶段补全；省略参数时会进入第二阶段参数输入。确认参数后，当前节点整棵子树才会被删除，并在相同位置插入具有新 ID 的新模块；取消输入不会修改 AST。

## 项目结构

```text
openscad-tui/
├── crates/
│   ├── core/       # AST、表达式解析和 OpenSCAD 代码生成
│   ├── assembly/   # 刚性零件装配模型、层级解析和多节点 DAE 导出
│   ├── library/    # 内置/外部模块与函数元数据
│   ├── render/     # Mesh/RenderScene、相机、CPU 光栅化和异步渲染服务
│   ├── terminal/   # 双缓冲终端编码、协议后端和 Ratatui 展示适配
│   └── ui/         # TUI、输入、命令和应用状态
├── stdlib.json     # 程序内部使用的 OpenSCAD 内建签名表
└── Cargo.toml      # Cargo workspace
```

## 开发与验证

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo check --workspace
```

测试覆盖 core、assembly、library、render、terminal 和 ui crate；实际数量以
`cargo test --workspace` 输出为准。

## 当前限制

- 模型预览依赖外部 `openscad` 可执行文件；项目不内嵌 OpenSCAD
- 模型预览需要手动触发；AST 更新只会将已有预览标记为过期
- `.scad` 导入器覆盖常用 declarations、module/function definitions、模块调用、容器、
  单子节点调用和局部赋值；尚未支持的复杂表达式会作为 AST 中的 raw expression 保留
- 能从调用文件相对路径解析到的 `.scad` 依赖会嵌入项目；只能通过 `OPENSCADPATH`、
  OpenSCAD 内置库或安装库找到的依赖仍保持外部引用
- STL、OFF、DXF、SVG、PNG 等由 `import` / `surface` 引用的非 SCAD 资源尚未嵌入项目
- Assembly 当前只支持刚性白膜零件、父子层级、TRS/pivot 和可见性；不提供顶点编辑、
  UV/材质、骨骼、蒙皮、动画、物理或 DAE 导入/预览
- 不是自由文本源码编辑器，主要通过 AST 树和命令编辑
- 函数参数默认值、语义类型检查和高级补全仍有限
- README 描述的是当前代码状态，项目尚未提供稳定发布或安装包

## License

MIT
