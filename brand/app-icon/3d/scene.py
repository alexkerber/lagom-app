# Render an icon concept: Blender -b -P scene.py -- <concept> <out.png>
import bpy, bmesh, sys, math
from mathutils import Vector
concept, out = sys.argv[sys.argv.index("--") + 1:][:2]

def hexc(h, a=1.0):
    h = h.lstrip("#"); c = [int(h[i:i+2], 16) / 255 for i in (0, 2, 4)]
    return [(x / 12.92) if x <= 0.04045 else ((x + 0.055) / 1.055) ** 2.4 for x in c] + [a]
PAPER, FALU, INK, CREAM, MUSTARD, PINE = "#F4EFE6", "#B03A2E", "#1C1B19", "#FBF7EE", "#D9A441", "#1E6B4E"

bpy.ops.wm.read_factory_settings(use_empty=True)
sc = bpy.context.scene
sc.render.engine = "CYCLES"; sc.cycles.samples = 96; sc.cycles.use_denoising = True
sc.render.resolution_x = sc.render.resolution_y = 1024
sc.view_settings.view_transform = "AgX"; sc.view_settings.look = "AgX - Medium High Contrast"

mats = {}
def mat(h, rough=0.45):
    if (h, rough) in mats: return mats[(h, rough)]
    m = bpy.data.materials.new(h); m.use_nodes = True
    b = m.node_tree.nodes["Principled BSDF"]; b.inputs["Base Color"].default_value = hexc(h); b.inputs["Roughness"].default_value = rough
    mats[(h, rough)] = m; return m

def box(size, loc, color, rot=(0, 0, 0), bevel=0.04, rough=0.45):
    bpy.ops.mesh.primitive_cube_add(location=loc, rotation=[math.radians(r) for r in rot])
    o = bpy.context.object; o.scale = [s / 2 for s in size]
    bpy.ops.object.transform_apply(scale=True)
    if bevel:
        m = o.modifiers.new("b", "BEVEL"); m.width = bevel; m.segments = 6; m.limit_method = "NONE"
    o.data.materials.append(mat(color, rough)); bpy.ops.object.shade_smooth(); return o

def tray(w, d, h, wall, color, y=0, bevel=0.04):
    # Open-top box from a floor and four walls; booleans on bevelled meshes were unreliable.
    box((w, d, wall), (0, y, wall / 2), color, bevel=bevel)
    box((w, wall, h), (0, y - d / 2 + wall / 2, h / 2), color, bevel=bevel)
    box((w, wall, h), (0, y + d / 2 - wall / 2, h / 2), color, bevel=bevel)
    box((wall, d, h), (-w / 2 + wall / 2, y, h / 2), color, bevel=bevel)
    box((wall, d, h), (w / 2 - wall / 2, y, h / 2), color, bevel=bevel)

# Floor in the icon background colour, catching soft shadows.
bpy.ops.mesh.primitive_plane_add(size=40); bpy.context.object.is_shadow_catcher = True
sc.render.film_transparent = True
w = bpy.data.worlds.new("w"); sc.world = w; w.use_nodes = True
w.node_tree.nodes["Background"].inputs[0].default_value = hexc("#F4EFE6"); w.node_tree.nodes["Background"].inputs[1].default_value = 0.6

def light(loc, energy, size):
    bpy.ops.object.light_add(type="AREA", location=loc); l = bpy.context.object
    l.data.energy = energy; l.data.size = size
    l.rotation_euler = (Vector((0, 0, 0.3)) - Vector(loc)).to_track_quat("-Z", "Y").to_euler()
light((-4, -3, 7), 900, 5); light((5, -5, 3), 220, 6)

def envelope_top(z, hw, hd):
    # Flap triangle and a red seal on the top envelope.
    bm = bmesh.new(); vs = [bm.verts.new(v) for v in ((-hw, hd, 0), (hw, hd, 0), (0, -0.1, 0))]; bm.faces.new(vs)
    me = bpy.data.meshes.new("flap"); bm.to_mesh(me); fl = bpy.data.objects.new("flap", me); sc.collection.objects.link(fl)
    fl.location = (0, 0, z); fl.modifiers.new("s", "SOLIDIFY").thickness = 0.014; fl.data.materials.append(mat("#E6DCC6", 0.7))
    bpy.ops.mesh.primitive_cylinder_add(radius=0.17, depth=0.04, location=(0, -0.1, z + 0.025)); bpy.context.object.data.materials.append(mat(FALU, 0.35))
target = Vector((0, 0, 0.45))
if concept == "kortladan":
    tray(2.5, 1.7, 1.0, 0.14, FALU, y=0.15, bevel=0.05)
    tabs = [(INK, -0.55), (MUSTARD, 0.15), (PINE, 0.75)]
    for i, (tc, tx) in enumerate(tabs):
        y = 0.55 - i * 0.38; h = 1.05 + 0.05 * i
        box((2.2, 0.035, h), (0, y, 0.12 + h / 2), CREAM, rot=(-6 + 3 * i, 0, 0), bevel=0.012, rough=0.6)
        box((0.5, 0.04, 0.24), (tx, y + 0.02 * math.sin(math.radians(-6 + 3 * i)), 0.12 + h + 0.09), tc, rot=(-6 + 3 * i, 0, 0), bevel=0.02, rough=0.5)
    box((0.7, 0.05, 0.28), (0, -0.72, 0.62), "#C9A45C", bevel=0.02, rough=0.25)       # brass label holder
    box((0.58, 0.05, 0.18), (0, -0.735, 0.62), CREAM, bevel=0.01, rough=0.7)
    target = Vector((0, 0.1, 0.6)); cam = (0, -5.6, 5.4)
elif concept == "brevbrickan":
    tray(2.7, 2.0, 0.42, 0.13, FALU, bevel=0.05)
    for i in range(5):
        box((2.2, 1.5, 0.045), (0, 0, 0.12 + 0.05 * i), CREAM if i % 2 else "#F1EADB", bevel=0.012, rough=0.65)
    envelope_top(0.12 + 0.05 * 4 + 0.025, 1.1, 0.75)
    target = Vector((0, 0, 0.2)); cam = (0, -3.9, 6.4)
else:  # stapeln: mess at the bottom, order on top
    spins = [(17, 0.28, -0.18), (-13, -0.25, 0.14), (9, 0.18, 0.1), (-4, -0.06, -0.04), (0, 0, 0), (0, 0, 0)]
    for i, (rz, dx, dy) in enumerate(spins):
        box((2.0, 1.4, 0.07), (dx, dy, 0.04 + 0.08 * i), CREAM if i % 2 else "#EDE5D3", rot=(0, 0, rz), bevel=0.02, rough=0.6)
    envelope_top(0.04 + 0.08 * 5 + 0.036, 1.0, 0.7)
    target = Vector((0, 0, 0.25)); cam = (0, -4.0, 6.2)

bpy.ops.object.camera_add(location=cam); c = bpy.context.object; sc.camera = c; c.data.lens = 70
c.rotation_euler = (target - c.location).to_track_quat("-Z", "Y").to_euler()
sc.render.filepath = out; bpy.ops.render.render(write_still=True)
