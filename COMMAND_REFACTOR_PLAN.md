# Command and Completion Refactor Plan

Status: temporary implementation document. Keep this file until the refactor is complete and
the durable user documentation has been updated.

Date: 2026-07-22

## Why this refactor exists

OpenSCAD TUI started with Vim-like, single-file commands such as `edit`, `write`, and `view`.
The application now owns a project containing editable SCAD sources, embedded libraries,
rendered models, and independent multi-part assemblies. The old flat command vocabulary no
longer describes those resources accurately.

The new command system must satisfy these invariants:

- Commands are grouped by the resource they operate on.
- Every capability exposed by the UI is backed by a command.
- The same command handlers can later be reused by a non-interactive CLI.
- Completion, execution, help, usage validation, and shortcuts consume one command schema.
- SCAD module, function, variable, expression, and parameter completion must not regress.
- The project is unpublished, so obsolete commands and compatibility aliases should be removed.
- Project save operations and source/model export operations remain distinct concepts.

## Command vocabulary

The general grammar is:

```text
<resource> <action> [target] [arguments] [options]
```

Resource namespaces:

```text
project   project lifecycle, persistence, and whole source-tree export
source    editable, project-owned SCAD sources
library   embedded, read-only SCAD dependencies
model     rendered results and standalone mesh files
assembly  rigid multi-part scenes and hierarchical DAE export
camera    model-preview camera operations
display   terminal rendering protocol and visual overlays, if introduced
screen    UI navigation only, if an explicit screen command remains necessary
```

AST modeling operations such as `insert`, `replace`, `set`, `unset`, `yank`, `paste`,
`translate`, `rotate`, and `scale` remain concise top-level commands. They operate on the
selected AST node rather than on a project resource.

### Planned project commands

```text
project new [name] [--force]
project open <file.scadtui>
project save [--force]
project save-as <file.scadtui> [--force]
project rename <name>
project export-sources <directory>
```

`project rename` changes persisted project metadata. `project save-as` changes the package
location. These operations must not be conflated.

### Planned source commands

```text
source new <name>
source import <file.scad>
source list
source switch <source>
source next
source previous
source rename <source> <name>
source remove <source>
source use <source>
source include <source>
source export <file.scad>
```

`source new head` appends the default `.scad` extension. Imported sources are embedded into the
project and decoupled from their original disk files. A source rename must atomically update
embedded virtual paths, active/entry source references, dependency edges, generated `use` and
`include` references, assembly mesh source references, and preview caches.

### Planned library commands

```text
library load <file.scad>
library list
library remove <library>
```

Loading embeds a library but does not add a `use` or `include` directive. Those directives are
explicit source operations.

### Planned model commands

```text
model render
model preview [--render]
model view <file.off|file.stl|file.dae>
model export <artifact>
```

Meanings:

- `model render` renders the currently selected editable source.
- `model preview` enters the cached model preview.
- `model preview --render` renders first and then enters the preview.
- `model view` opens a standalone mesh file without importing it into the project.
- `model export` exports the currently selected source as a flat model.

The obsolete top-level `render`, `preview`, `view`, and `export` commands should be removed after
all shortcuts and startup paths use the new commands or shared services.

### Planned assembly commands

The existing `assembly` namespace is conceptually correct and remains separate from `model`:

```text
assembly new [name]
assembly open <assembly>
assembly list
assembly add <source> [name]
assembly select <part>
assembly copy [part]
assembly paste [parent|root]
assembly remove [part]
assembly parent [part] <parent|root>
assembly translate [part] <x> <y> <z>
assembly rotate [part] <x> <y> <z>
assembly scale [part] <x> <y> <z>
assembly pivot [part] <x> <y> <z>
assembly visibility [part] <show|hide|toggle>
assembly render
assembly preview
assembly export <file.dae>
assembly close
```

`model export scene.dae` produces a single flat mesh. `assembly export scene.dae` preserves
multiple meshes, node names, hierarchy, and transforms.

## Command schema architecture

The current registry is flat and resolves only the first token. `CommandType` then routes input
through hard-coded completion branches. Replace this with a hierarchical command tree whose leaf
nodes contain execution and argument metadata.

Conceptual data model:

```rust
struct CommandSpec {
    path: &'static [&'static str],
    aliases: &'static [&'static [&'static str]],
    description: &'static str,
    arguments: Vec<ArgumentSpec>,
    handler: CommandHandler,
    changes_project: bool,
    write_to_history: bool,
    examples: &'static [&'static str],
}

enum ArgumentSpec {
    Literal(&'static [&'static str]),
    Path(PathSpec),
    ProjectSource(SourceFilter),
    Library,
    Assembly,
    AssemblyPart,
    Module,
    ModuleParameters,
    NodeParameter,
    Expression(ExpressionContext),
    String,
    Float,
    Variadic(Box<ArgumentSpec>),
    Optional(Box<ArgumentSpec>),
}
```

The exact Rust representation may differ, but behavior must follow these rules:

1. Tokenize command input once, preserving token byte ranges for completion replacement.
2. Resolve the longest matching command path.
3. Complete child command nodes until a leaf command is reached.
4. Complete leaf arguments using declarative static or dynamic providers.
5. Pass only arguments after the matched command path to the handler.
6. Generate validation errors, usage text, examples, and help from the same specification.

The tokenizer should understand quoted arguments and escaped whitespace. The current executor's
`split_whitespace` behavior cannot represent paths containing spaces and must not be carried into
the new architecture.

## Completion design

Completion has two layers:

### Command-language completion

This layer is owned by the command registry:

- root resource/command completion;
- child action completion;
- static literal options;
- filesystem paths with extension filters;
- project sources, loaded libraries, assemblies, and assembly parts;
- replacement ranges derived from tokenizer spans rather than fixed token indexes.

### OpenSCAD-language completion

This layer keeps the existing AST/library-backed providers:

- built-in and parsed modules;
- module parameters and default values;
- global variables and functions;
- expression fragments, lists, nested lists, and function calls;
- existing definition names for redefinition;
- selected-node parameter names and current values.

The registry selects the appropriate OpenSCAD provider through `ArgumentSpec`; it must not embed
module/function lookup logic directly in the command tree.

Dynamic completion providers receive read-only application context and the parsed argument state.
They must not mutate the project.

## Help and discoverability

Help is generated recursively from the command tree:

```text
help
help model
help model view
help source rename
```

`help model` lists child actions. `help model view` shows the leaf description, arguments,
accepted extensions, and examples. Completion for `help` follows the same tree.

The status line and completion popup should display short descriptions when available, but command
execution must not depend on the UI presentation.

## UI and shortcut integration

Keyboard shortcuts and clickable UI controls dispatch canonical command strings through the same
registry as typed commands. No shortcut may call a handler directly when the equivalent operation
is expected to be scriptable.

Expected migrations include:

```text
render                  -> model render
preview model           -> model preview
view asset.stl          -> model view asset.stl
export model out.stl    -> model export out.stl
export source out.scad  -> source export out.scad
export tree directory   -> project export-sources directory
new project             -> project new
new file part           -> source new part
edit part.scad          -> source import part.scad
buffer part.scad        -> source switch part.scad
library path.scad       -> library load path.scad
use library.scad        -> source use library.scad
include library.scad    -> source include library.scad
write                   -> project save
write!                  -> project save --force
open file.scadtui       -> project open file.scadtui
```

No old aliases are required because the project has not been released.

## Export format architecture

Introduce an internal export format registry shared by model and assembly services. Format is
normally inferred from the destination extension and validated before invoking OpenSCAD or an
internal writer.

The registry records:

- canonical extension and aliases;
- whether OpenSCAD performs the export;
- whether OpenSCAD TUI has an internal writer;
- whether the format supports a flat mesh, hierarchy, transforms, or multiple meshes;
- availability checks and actionable errors.

`model export` and `assembly export` share format detection and diagnostics but remain separate
operations because their data models differ.

## Migration phases

1. Add tokenizer, hierarchical registry, command resolution, and focused unit tests.
2. Route execution, completion, help, and argument validation through the new registry.
3. Preserve advanced OpenSCAD completion providers behind declarative argument specifications.
4. Migrate project, source, library, model, camera/display, and assembly command definitions.
5. Update shortcuts, UI controls, startup file handling, README, and help examples.
6. Delete flat `CommandType` routing, bespoke `New`/`Export`/`Assembly` completion branches, and
   obsolete command registrations.
7. Run formatting, Clippy with warnings denied, the complete workspace test suite, release build,
   and `cargo install --path . --locked` verification.
8. Replace this temporary document with durable command reference documentation or remove it once
   all requirements have landed.

## Required regression coverage

- Longest-path execution dispatches `model view` separately from `model export`.
- Partial completion works at every command-tree level.
- Completion replacement changes only the token under the cursor.
- Quoted paths and paths containing spaces execute and complete correctly.
- File completion applies extension filters without hiding directories.
- Source, library, assembly, and part completion reflect current project state.
- Module, parameter, function, global, expression, list, and nested-list completion remain intact.
- `help <namespace>` and `help <leaf path>` match executable commands.
- Every shortcut command resolves through the registry.
- Old top-level commands are rejected after migration.
- `model export` and `assembly export` select the correct pipelines.
- Dirty-state and command-history policies remain correct for every migrated command.

## Completion criteria

The refactor is complete when commands, completion, help, UI shortcuts, and documentation all use
the hierarchical resource vocabulary; no obsolete flat routing remains; all regression tests pass;
and the temporary plan has either been converted into maintained documentation or removed.
