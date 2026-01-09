# Library System - Include Statement Handling

## How It Works

### Module Source Tracking

When you insert a module into your design, the system automatically tracks where that module comes from:

```
Built-in modules (StandardLibrary)
├─ cube, sphere, cylinder
├─ square, circle
├─ translate, rotate, scale, mirror
└─ union, difference, intersection, hull, minkowski, render
    → source_library = None
    → No include statement

Third-party library modules
└─ Custom modules from loaded libraries
    → source_library = Some("LibraryName")
    → Include statement auto-added
```

### Include Statement Auto-Generation

**When you load a third-party library:**

```bash
library custom_shapes.json
```

File content: `custom_shapes.json`
```json
{
  "name": "CustomShapes",
  "file": "custom_shapes.scad",  ← This is the include path
  "modules": [
    { "name": "rounded_cube", ... },
    { "name": "hollow_cylinder", ... }
  ]
}
```

**When you insert a module from that library:**

```
insert rounded_cube size=10
```

The system automatically:
1. Records `source_library = Some("CustomShapes")`
2. Adds `"custom_shapes.scad"` to `AST.includes`

**When you export:**

```
export my_design.scad
```

Generated code:
```scad
include <custom_shapes.scad>;

rounded_cube(size=10);
```

### Multiple Libraries

You can load multiple libraries, and includes are automatically managed:

```bash
library shapes.json        # file: "shapes.scad"
library mechanical.json    # file: "mechanical.scad"
```

Insert modules from both:
```
insert custom_gear teeth=20
insert special_bolt diameter=5
```

Exported code:
```scad
include <shapes.scad>;
include <mechanical.scad>;

custom_gear(teeth=20);
special_bolt(diameter=5);
```

**Duplicate includes are prevented** - if you use multiple modules from the same library, the include statement appears only once.

### Important Notes

#### Built-in Library (StandardLibrary)

The `StandardLibrary` (loaded from `stdlib.json`) is **special**:
- It's either embedded in the binary or loaded from user config
- Modules have `source_library = None`
- No include statements are generated
- Built-in modules are assumed to be available in all OpenSCAD installations

#### Third-party Libraries

Any library loaded via the `library` command:
- Has `source_library = Some(library_name)`
- Generates include statements based on the `file` field
- Multiple modules from the same library share one include statement

### AST Structure

```rust
pub struct AstRoot {
    pub modules: Vec<ModuleNode>,
    pub includes: Vec<String>,  ← Auto-populated with library files
    pub uses: Vec<String>,      ← Reserved for future use
}

pub struct ModuleNode {
    pub name: String,
    pub source_library: Option<String>,  ← None or library name
    pub children: Vec<ModuleNode>,
    // ...
}
```

### Workflow Example

1. **Start fresh:**
   ```
   $ ./openscad-tui
   AST.includes = []
   ```

2. **Load third-party library:**
   ```
   Shift+L → library_example.json
   AST.includes = []  (no include yet, library just loaded)
   ```

3. **Insert module from library:**
   ```
   i rounded_cube size=10
   AST.includes = ["library_example.scad"]
   Node: rounded_cube
     source_library = Some("ExampleLibrary")
   ```

4. **Insert built-in module:**
   ```
   i cube size=5
   AST.includes = ["library_example.scad"]  (unchanged)
   Node: cube
     source_library = None
   ```

5. **Insert another module from same library:**
   ```
   i hollow_cylinder diameter=3
   AST.includes = ["library_example.scad"]  (still one include!)
   Node: hollow_cylinder
     source_library = Some("ExampleLibrary")
   ```

6. **Export code:**
   ```
   Ctrl+E → design.scad
   
   Generated code:
   include <library_example.scad>;
   
   rounded_cube(size=10);
   cube(size=5);
   hollow_cylinder(diameter=3);
   ```

### Save/Load Behavior

When you save the project with `w`:
```
write my_project.json
```

Saved JSON includes:
- All module nodes with their `source_library` field
- All include statements in `AST.includes`

When you load with `e`:
```
edit my_project.json
```

The include statements are preserved, ready for export.

**Note:** You still need to have the library files available when exporting or using in OpenSCAD. The application just ensures the include statements are correct.

