# Looking Glass NG Architecture

## 1. Overview

Looking Glass NG is a Wayland compositor with a spatial 3D window-management model.

Applications remain conventional Wayland clients.

The compositor converts application surfaces into renderable objects in a 3D scene.

High-level architecture:

Applications
    |
    v
Wayland protocol
    |
    v
+---------------------------+
| Looking Glass Compositor  |
|                           |
|  Wayland State            |
|       |                   |
|       v                   |
|  Window Manager           |
|       |                   |
|       v                   |
|  Scene Graph              |
|       |                   |
|       +---- Camera         |
|       |                   |
|       v                   |
|  Renderer                 |
|       |                   |
+-------|-------------------+
        v
     GPU / DRM
        |
        v
     Display

---

# 2. Major subsystems

src/

compositor/
    Wayland protocol integration

window/
    Window-management abstraction

scene/
    3D world representation

renderer/
    GPU rendering

input/
    Keyboard/pointer handling

layout/
    Spatial placement algorithms

ui/
    Compositor-owned UI

config/
    Configuration

backend/
    Backend-specific integration where necessary

---

# 3. Central state

The compositor should have a central state object.

Conceptually:

struct State {
    display;
    compositor;
    shell;
    seat;
    outputs;
    windows;
    workspaces;
    scene;
    input;
    renderer;
    config;
}

The exact Smithay types will depend on the current version.

Do not blindly copy this pseudo-code.

---

# 4. Window model

A Window represents an application-level desktop window.

Conceptually:

Window
{
    id
    surface
    app_id
    title

    lifecycle
    workspace

    logical_geometry

    visual
}

Visual:

{
    position
    rotation
    scale
    opacity
}

The Window does not contain OpenGL objects.

---

# 5. Surface model

A Wayland surface may have:

- subsurfaces
- damage
- buffer state
- input regions
- opaque regions
- transforms

Smithay's compositor helpers maintain coherent surface-tree state.

The renderer should consume that information rather than reconstructing protocol state.

---

# 6. Scene graph

The scene graph represents visual objects.

Conceptually:

Scene
|
+-- Background
|
+-- Workspace
|   |
|   +-- WindowNode
|   +-- WindowNode
|   +-- WindowNode
|
+-- Workspace
|
+-- CompositorUI

A WindowNode references a Window but does not own the Window.

---

# 7. Transform model

Every spatial node can have:

position: Vec3
rotation: Quaternion
scale: Vec3

Transformation order:

local
→ scale
→ rotation
→ translation
→ parent transform
→ camera transform
→ projection

Use matrices at the renderer boundary.

---

# 8. Camera

The Camera represents the user's view into the world.

Properties:

position
rotation
field_of_view
near_plane
far_plane

Matrices:

view_matrix
projection_matrix

The camera belongs to an output/view.

---

# 9. Output model

An Output represents a physical or nested display.

Eventually:

Output
{
    physical_output
    viewport
    camera
    framebuffer
}

Initially there is only one output.

---

# 10. Coordinate spaces

The following coordinate spaces must remain explicit.

## Surface coordinates

Coordinates inside an application buffer.

## Window coordinates

Coordinates relative to the logical window.

## Workspace coordinates

Coordinates relative to a workspace.

## World coordinates

Coordinates in the global 3D scene.

## Camera coordinates

Coordinates after view transformation.

## Clip coordinates

Coordinates after projection.

## Screen coordinates

2D output coordinates.

Conversions must be explicit.

---

# 11. Input pipeline

Pointer:

screen position
    |
    v
normalized device coordinates
    |
    v
inverse projection
    |
    v
inverse view
    |
    v
world-space ray
    |
    v
scene intersection
    |
    v
WindowNode
    |
    v
window-local coordinates
    |
    v
Wayland pointer event

---

# 12. Ray intersection

For MVP, every window can be represented as a rectangular plane.

Intersection:

Ray
    |
    +---- Window A
    |
    +---- Window B
    |
    +---- Window C

Select the closest valid intersection.

Later support:

- rotated windows
- tilted windows
- arbitrary surfaces
- compositor UI
- depth occlusion

---

# 13. Rendering

The renderer receives a scene and camera.

Conceptual frame:

begin_frame()

update_camera()

collect_visible_nodes()

sort/prepare_nodes()

render_background()

render_shadows()

render_windows()

render_compositor_ui()

end_frame()

present()

---

# 14. Window rendering

A window consists conceptually of:

shadow
frame/decorations
application surface
optional back surface

The application surface is a GPU texture.

Do not modify application pixels merely to create 3D effects.

---

# 15. Window backs

Looking Glass NG supports a second compositor-owned surface behind a window.

Front:

application surface

Back:

compositor-generated information

Example:

Back
{
    application name
    workspace
    PID
    window state
    actions
}

The initial implementation may simply render a generated panel.

---

# 16. Animation architecture

Animations operate on visual state.

Animation:

{
    start
    target
    duration
    elapsed
    easing
}

The logical state must not depend on animation completion.

For example, a window can logically move to workspace 2 while its visual animation is still running.

---

# 17. Workspace model

A workspace is a logical collection of windows.

Workspace:

{
    id
    name
    windows
    world_position
}

Initially workspace position may be unused.

Eventually workspaces can occupy different areas in 3D space.

---

# 18. Spatial mode

Two major modes:

NORMAL
SPATIAL

NORMAL:

- camera close
- windows mostly planar
- conventional interaction

SPATIAL:

- camera moves backward
- perspective becomes obvious
- workspaces/windows become navigable spatially

Do not duplicate window state between modes.

Only change camera/visual presentation.

---

# 19. Overview

Overview mode is a camera transformation plus a layout operation.

It should not create duplicate application windows.

Flow:

current scene
    |
    v
compute overview layout
    |
    v
animate camera
    |
    v
animate windows if necessary
    |
    v
interactive overview
    |
    v
selection
    |
    v
return camera to selected window

---

# 20. Spatial Alt-Tab

Alt-Tab should use the same scene/layout infrastructure.

Do not create a completely separate rendering system.

Flow:

input
    |
    v
switcher state
    |
    v
select windows
    |
    v
temporary layout
    |
    v
camera
    |
    v
selection
    |
    v
restore previous camera

---

# 21. Layout engine

LayoutEngine is independent of rendering.

Trait concept:

LayoutEngine::layout(windows, viewport) -> Vec<Transform3D>

Implement:

Grid
Wall
Stack
Arc
Circle
Tile
Freeform

Each layout should be deterministic.

---

# 22. Renderer abstraction

Conceptually:

trait Renderer {
    begin_frame()
    render_scene()
    end_frame()
}

OpenGL renderer implements it.

A future Vulkan renderer can implement the same conceptual interface.

The exact Rust trait should be designed around actual Smithay renderer requirements rather than forcing an artificial abstraction.

---

# 23. Texture lifecycle

Conceptually:

Wayland Buffer
    |
    v
Buffer Import
    |
    v
GPU Texture
    |
    v
Texture Cache

The texture cache must react correctly to buffer replacement/destruction.

Do not keep GPU resources alive indefinitely.

---

# 24. Damage

Initial implementation:

full-frame rendering

Later:

surface damage
    |
    v
window damage
    |
    v
scene damage
    |
    v
output damage

Damage optimization should never change visual correctness.

---

# 25. XWayland

Architecture:

X11 client
    |
    v
XWayland
    |
    v
Wayland surface
    |
    v
Window
    |
    v
Scene

X11-specific behavior must remain isolated.

---

# 26. Configuration

Configuration is loaded from:

~/.config/looking-glass/config.toml

Possible sections:

display
camera
animation
input
effects
workspaces
keybindings

Configuration changes should not require recompilation.

---

# 27. Persistence

Future persistence should save:

- application identity
- workspace
- preferred position
- preferred rotation
- preferred scale

Do not save transient state such as:

- animation progress
- GPU handles
- Wayland object IDs

---

# 28. Security

Application clients must not be able to:

- manipulate arbitrary windows
- read arbitrary surfaces
- access compositor internals
- move the camera
- inspect another application's private state

Compositor-owned features remain privileged.

---

# 29. Performance targets

Initial:

60 FPS

Target:

120 FPS

Advanced:

144/165/240 FPS

Important metrics:

- frame time
- compositor CPU time
- GPU frame time
- input latency
- surface upload time
- texture memory
- number of visible nodes

---

# 30. Development backend

The compositor must support nested development.

Preferred development path:

Existing desktop
    |
    v
Nested Looking Glass
    |
    v
Test applications

Only later:

TTY
    |
    v
DRM
    |
    v
Looking Glass

This protects the developer's main desktop during development.

---

# 31. Architecture invariants

The following must remain true:

1. Wayland applications remain unmodified.
2. Window state is independent of rendering.
3. Scene state is independent of protocol callbacks.
4. Renderer does not own window-management decisions.
5. Input resolves through the scene.
6. 3D rotation uses quaternions.
7. Animations modify visual state, not protocol state.
8. Workspaces are logical entities.
9. Spatial mode is a presentation/navigation mode.
10. Full-frame rendering may be used initially.
11. GPU resource ownership is explicit.
12. Protocol correctness is never sacrificed for visual effects.