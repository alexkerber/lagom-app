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
light((-4, -3, 7), 1100, 6); light((5, -4, 3), 260, 6); light((1, 6, 4), 500, 3)

def envelope_top(z, hw, hd):
    # Flap triangle and a red seal on the top envelope.
    bm = bmesh.new(); vs = [bm.verts.new(v) for v in ((-hw, hd, 0), (hw, hd, 0), (0, -0.1, 0))]; bm.faces.new(vs)
    me = bpy.data.meshes.new("flap"); bm.to_mesh(me); fl = bpy.data.objects.new("flap", me); sc.collection.objects.link(fl)
    fl.location = (0, 0, z); fl.modifiers.new("s", "SOLIDIFY").thickness = 0.014; fl.data.materials.append(mat("#E6DCC6", 0.7))
    bpy.ops.mesh.primitive_cylinder_add(radius=0.17, depth=0.04, location=(0, -0.1, z + 0.025)); bpy.context.object.data.materials.append(mat(FALU, 0.35))

BLUE = "#2F56A8"
def metal(h="#B9BBBD"):
    m = bpy.data.materials.new("metal"); m.use_nodes = True; b = m.node_tree.nodes["Principled BSDF"]
    b.inputs["Base Color"].default_value = hexc(h); b.inputs["Metallic"].default_value = 1.0; b.inputs["Roughness"].default_value = 0.28
    return m
def curl(loc, size, angle, color, rot=(0, 0, 0), axis="X"):
    # A paper sheet bent into a curl.
    bpy.ops.mesh.primitive_plane_add(location=loc, rotation=[math.radians(r) for r in rot]); o = bpy.context.object
    o.scale = (size[0] / 2, size[1] / 2, 1); bpy.ops.object.transform_apply(scale=True)
    bpy.ops.object.mode_set(mode="EDIT"); bpy.ops.mesh.subdivide(number_cuts=40); bpy.ops.object.mode_set(mode="OBJECT")
    m = o.modifiers.new("bend", "SIMPLE_DEFORM"); m.deform_method = "BEND"; m.angle = math.radians(angle); m.deform_axis = axis
    o.modifiers.new("t", "SOLIDIFY").thickness = 0.012
    o.data.materials.append(mat(color, 0.7)); bpy.ops.object.shade_smooth(); return o
def rod(p0, p1, r, m):
    p0, p1 = Vector(p0), Vector(p1); d = p1 - p0
    bpy.ops.mesh.primitive_cylinder_add(radius=r, depth=d.length, location=(p0 + p1) / 2, vertices=48)
    o = bpy.context.object; o.rotation_euler = d.to_track_quat("Z", "Y").to_euler()
    bev = o.modifiers.new("b", "BEVEL"); bev.width = min(r * 0.6, 0.06); bev.segments = 6
    o.data.materials.append(m); bpy.ops.object.shade_smooth(); return o

if concept == "osthyvel":
    for i in range(10):  # a ream of paper: the backlog
        box((2.3, 1.7, 0.09), (0, 0, 0.045 + 0.09 * i), CREAM if i % 2 else "#EFE7D6", bevel=0.01, rough=0.75)
    top = 0.9; steel = metal()
    rear = box((1.5, 0.78, 0.03), (0.05, 0.12, top + 0.035), CREAM, bevel=0.01); rear.data.materials[0] = steel
    front = box((1.5, 0.16, 0.03), (0.05, -0.47, top + 0.035), CREAM, bevel=0.01); front.data.materials[0] = steel
    # The slice: an Archimedean spiral rising out of the slot and curling back over the blade.
    bm = bmesh.new(); y0, z0, r0, w = -0.38, top + 0.05, 0.3, 0.62
    prev = None
    for k in range(121):
        t = k / 120 * 1.75 * math.pi; r = r0 * (1 - 0.32 * t / math.pi)
        y = y0 + r * math.sin(t); z = z0 + r0 - r * math.cos(t)
        a, b = bm.verts.new((0.05 - w, y, z)), bm.verts.new((0.05 + w, y, z))
        if prev: bm.faces.new((prev[0], prev[1], b, a))
        prev = (a, b)
    me = bpy.data.meshes.new("slice"); bm.to_mesh(me); sl = bpy.data.objects.new("slice", me); sc.collection.objects.link(sl)
    sl.modifiers.new("t", "SOLIDIFY").thickness = 0.018; sl.data.materials.append(mat("#FFF9EC", 0.6))
    for p in me.polygons: p.use_smooth = True
    rod((0.05, 0.5, top + 0.04), (0.05, 1.05, top + 0.35), 0.05, steel)
    rod((0.05, 1.05, top + 0.35), (0.05, 2.05, top + 0.6), 0.18, mat(FALU, 0.35))
    target = Vector((-0.15, 0.25, 0.55)); cam = (5.2, -5.4, 4.9)
elif concept == "kladnypa":
    for i, (c, rz, dx) in enumerate([(BLUE, 7, -0.12), (MUSTARD, -4, 0.1), (CREAM, 0, 0)]):
        box((2.0, 1.5, 0.03), (dx, -0.25, 0.02 + 0.035 * i), c, rot=(0, 0, rz), bevel=0.008, rough=0.7)
    wood = mat(FALU, 0.5); z0 = 0.06
    box((0.34, 2.4, 0.16), (0, 0.55, z0 + 0.2), FALU, rot=(-3, 0, 0), bevel=0.06, rough=0.5)   # top jaw
    box((0.34, 2.4, 0.16), (0, 0.55, -0.02), FALU, bevel=0.06, rough=0.5)                     # bottom jaw, under the cards
    steel = metal()
    for k in range(5):
        bpy.ops.mesh.primitive_torus_add(major_radius=0.2, minor_radius=0.025, location=(0, 0.95 + k * 0.05, z0 + 0.1), rotation=(0, math.radians(90), 0))
        bpy.context.object.data.materials.append(steel)
    target = Vector((0, 0.25, 0.1)); cam = (2.6, -3.6, 6.2)
else:  # kam
    box((2.6, 0.55, 0.2), (0, 0.85, 0.22), FALU, bevel=0.07, rough=0.4)
    for tx in (-1.05, -0.35, 0.35, 1.05):
        box((0.3, 1.5, 0.16), (tx, -0.1, 0.2), FALU, bevel=0.07, rough=0.4)
    for sx, c in ((-0.7, BLUE), (0.0, MUSTARD), (0.7, PINE)):
        box((0.36, 2.0, 0.025), (sx, 0.1, 0.02), c, bevel=0.006, rough=0.65)
    for sx, c, rz in ((-0.7, BLUE, 14), (0.0, MUSTARD, -5), (0.7, PINE, -16)):
        box((0.36, 1.0, 0.025), (sx + math.sin(math.radians(rz)) * -0.5, -1.35, 0.03), c, rot=(0, 0, rz), bevel=0.006, rough=0.65)
    target = Vector((0, -0.15, 0.1)); cam = (1.2, -3.8, 6.6)

bpy.ops.object.camera_add(location=cam); c = bpy.context.object; sc.camera = c; c.data.lens = 70
c.rotation_euler = (target - c.location).to_track_quat("-Z", "Y").to_euler()
sc.render.filepath = out; bpy.ops.render.render(write_still=True)
