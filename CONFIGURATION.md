# Configuration

## Standard Library (stdlib.json)

### Default Behavior

The application includes a built-in standard library that is embedded at compile time. This ensures the application is completely self-contained and can be distributed as a single executable.

### Customization

To customize the standard library modules, you can place a `stdlib.json` file in your OpenSCAD TUI configuration directory:

**Linux/macOS:**
```
~/.config/openscad-tui/stdlib.json
```

**Windows:**
```
%APPDATA%\openscad-tui\stdlib.json
```

### How It Works

1. On startup, the application checks your config directory for `stdlib.json`
2. If found, it loads that file and uses it as the standard library
3. If not found, it uses the built-in embedded standard library
4. Any error loading the user config file is logged as a warning, and the embedded library is used as fallback

### Creating a Custom stdlib.json

You can modify the standard library by copying the embedded stdlib.json to your config directory and editing it:

```bash
# Create config directory if it doesn't exist
mkdir -p ~/.config/openscad-tui

# Copy the default stdlib.json (you'll need to extract it from this repository)
cp stdlib.json ~/.config/openscad-tui/stdlib.json

# Edit as needed
nano ~/.config/openscad-tui/stdlib.json
```

### Format

The stdlib.json file follows this schema:

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

### Module Properties

- **name**: Module identifier
- **description**: Human-readable description
- **accepts_children**: Whether the module can contain sub-elements
- **parameters**: Array of parameter definitions
  - **name**: Parameter name
  - **param_type**: Type (integer, float, string, boolean, list)
  - **default**: Default value (optional)
  - **description**: Parameter description (optional)

### Examples

#### Add a Custom Module

```json
{
  "name": "my_custom_box",
  "description": "Custom box with default dimensions",
  "accepts_children": false,
  "parameters": [
    {
      "name": "length",
      "param_type": "float",
      "default": "10",
      "description": "Box length"
    },
    {
      "name": "width",
      "param_type": "float",
      "default": "5",
      "description": "Box width"
    }
  ]
}
```

#### Override Existing Module

You can override built-in modules by including them in your custom stdlib.json with different parameters.

## Third-Party Libraries

In addition to the standard library, you can load third-party module libraries using the `library` command:

```
library ~/my_libraries/my_shapes.json
```

Or use the Shift+L keyboard shortcut to load libraries interactively.

See the project README for more details on library management.
