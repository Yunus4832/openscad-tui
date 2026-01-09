# OpenSCAD TUI - Library Management System

## 架构概述

### 模块加载流程

insert 命令支持的模块来自两个源：

1. **内建标准库** - 硬编码在代码中（crates/library/src/lib.rs）
   - cube, sphere, cylinder, square, circle
   - translate, rotate, scale, mirror
   - union, difference, intersection
   - hull, minkowski, render

2. **加载的第三方库** - 通过库文件加载（JSON 格式）
   - 用户可以创建自定义库文件
   - 通过 `library <filename>` 命令加载
   - 库中的模块与内建模块具有相同地位

### 完整流程

```
用户输入 insert 命令
    ↓
cmd_insert() → app.library.get_module(name)
    ↓
检查内建标准库 → 不存在则检查加载的库
    ↓
返回 ModuleDef → 创建模块实例
```

## 文件格式

### stdlib.json - 标准库定义

标准 OpenSCAD 模块的定义，存储在 `stdlib.json`：

```json
{
  "name": "StandardLibrary",
  "description": "OpenSCAD Standard Library Definitions",
  "file": "stdlib.scad",
  "version": "1.0",
  "modules": [
    {
      "name": "cube",
      "description": "Creates a cube",
      "accepts_children": false,
      "parameters": [
        {
          "name": "size",
          "param_type": "list",
          "default": "[1, 1, 1]",
          "description": "Cube dimensions [x, y, z]"
        }
      ]
    }
  ]
}
```

### 自定义库文件 - 第三方模块

用户可以创建自定义库文件（参考 `custom_library_example.json`）：

```json
{
  "name": "CustomLibrary",
  "description": "My Custom Modules",
  "file": "custom.scad",
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
          "default": "[10, 10, 10]"
        }
      ]
    }
  ]
}
```

## 命令参考

### 库加载命令

#### Shift+L 快捷键或 `library` 命令

加载第三方库文件：

```
库名: library custom_library.json
效果: 加载 custom_library.json 中的所有模块
```

示例步骤：
1. 按 Shift+L
2. 输入库文件名：`custom_library_example.json`
3. 按 Enter 确认
4. 成功消息：`✓ Loaded library from custom_library_example.json`

加载后，自定义库中的模块可以像内建模块一样使用 `insert` 命令添加。

### 项目文件操作

#### w 快捷键或 `write` 命令

保存项目到 JSON 文件：

```
项目保存: write my_project.json
```

#### e 快捷键或 `edit` 命令

从 JSON 文件加载项目：

```
项目加载: edit my_project.json
```

#### Ctrl+E 快捷键或 `export` 命令

导出为 OpenSCAD 代码：

```
代码导出: export design.scad
```

## 使用示例

### 场景 1：加载自定义库并使用

```
1. 启动应用
2. 按 Shift+L，输入 "custom_library_example.json"
3. 确认加载
4. 现在可以使用 insert 命令添加 rounded_cube, hollow_cylinder 等
5. 输入：i rounded_cube radius=2
```

### 场景 2：完整工作流

```
1. 按 L (小写) 展开树节点
2. 按 V 选择节点进行组合操作
3. 按 W 保存设计到 my_design.json
4. 按 Ctrl+E 导出 OpenSCAD 代码到 design.scad
5. 在 OpenSCAD 中打开 design.scad 渲染
```

### 场景 3：加载已保存的库组合

```
# 之前创建的库
library my_shapes.json

# 现在可以添加这些模块
insert gear teeth=24 pitch=5

# 保存整个项目
write my_mechanical_design.json
```

## 库文件创建指南

### 最小库定义

```json
{
  "name": "MyLib",
  "description": "My custom modules",
  "file": "my_lib.scad",
  "version": "1.0",
  "modules": [
    {
      "name": "my_module",
      "description": "My custom module",
      "accepts_children": false,
      "parameters": []
    }
  ]
}
```

### 参数类型

支持的参数类型：
- `integer` - 整数
- `float` - 浮点数
- `string` - 字符串
- `boolean` - 布尔值
- `list` - 列表

### accepts_children 字段

- `true` - 模块可以有子节点（如 translate, union）
- `false` - 模块不能有子节点（如 cube, sphere）

## 技术细节

### 数据流

```
stdlib.json
    ↓
LibraryManager::new() 
    → init_builtin_modules()
    ↓
app.library.builtin_modules (HashMap<String, ModuleDef>)

custom_library.json
    ↓
cmd_load_library(app, "custom_library.json")
    → app.library.load_library(path)
    ↓
app.library.libraries (HashMap<String, LibraryDef>)

get_module(name)
    ↓
检查 builtin_modules → 检查 libraries
    ↓
返回 ModuleDef
```

### 文件格式

所有配置文件使用 **JSON** 格式（serde_json）：
- 标准库：`stdlib.json`
- 项目文件：`*.json`
- 库文件：`*.json`

JSON 格式优点：
- ✅ 支持嵌套枚举（与 YAML 不同）
- ✅ 更好的嵌套结构支持
- ✅ 更严格的格式验证

## 常见问题

### Q: 如何确认库已加载？
A: 加载库后会显示 `✓ Loaded library from ...` 消息。使用 `insert <module>` 命令尝试添加库中的模块即可验证。

### Q: 内建模块可以被覆盖吗？
A: 不能。`get_module()` 先检查内建模块，所以同名的库模块会被忽略。这是设计特性，保护核心功能。

### Q: 可以加载多个库吗？
A: 可以。每次 `library` 命令加载一个库文件，所有加载的库会合并到 LibraryManager 中。

### Q: 如何卸载库？
A: 目前没有卸载命令。可以重启应用重新加载。

### Q: 库中的模块可以调用其他库中的模块吗？
A: 这需要在生成的 OpenSCAD 代码中正确引入库文件。目前应用只管理模块定义，不处理代码生成的依赖关系。

## 扩展计划

未来可能添加的功能：
- [ ] 库卸载命令
- [ ] 库列表查看命令 (list libraries)
- [ ] 库搜索功能 (search module <pattern>)
- [ ] 库版本管理
- [ ] 从网络加载库
- [ ] 库包管理系统
