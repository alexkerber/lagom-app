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
elif concept == "osthyvel2":
    for i in range(9):  # the backlog: a ream of paper
        box((2.1, 1.5, 0.1), (0, 0, 0.05 + 0.1 * i), CREAM if i % 2 else "#EFE7D6", bevel=0.012, rough=0.75)
    T = 0.9; steel = metal("#C4C7CA")
    def hw(x):  # half width of the blade along its length: broad spade, tapering to the neck
        return 0.52 if x <= -0.35 else 0.52 - (x + 0.35) / 0.6 * 0.34
    def poly(pts):
        bm = bmesh.new(); vs = [bm.verts.new((x, y, T + 0.02)) for x, y in pts]; bm.faces.new(vs)
        me = bpy.data.meshes.new("blade"); bm.to_mesh(me); o = bpy.data.objects.new("blade", me); sc.collection.objects.link(o)
        o.modifiers.new("t", "SOLIDIFY").thickness = 0.028; b = o.modifiers.new("b", "BEVEL"); b.width = 0.008; b.segments = 3
        o.data.materials.append(steel); return o
    r = 0.12  # rounded front corners
    front = [(-0.92 + r - r * math.cos(a), sgn * (0.52 - r + r * math.sin(a))) for sgn in (-1,) for a in [i * math.pi / 12 for i in range(7)]]
    lip = [(-0.62, -0.52)] + [(-0.92 + r - r * math.sin(a), -(0.52 - r) - r * math.cos(a)) for a in [i * math.pi / 12 for i in range(7)]][::-1]
    lip = [(-0.62, -0.52), (-0.80, -0.52)] + [(-0.92 + r - r * math.sin(a), -(0.52 - r) - r * math.cos(a)) for a in [i * math.pi / 12 for i in range(1, 7)]] + \
          [(-0.92 + r - r * math.sin(a), (0.52 - r) + r * math.cos(a)) for a in [i * math.pi / 12 for i in range(6, 0, -1)]] + [(-0.80, 0.52), (-0.62, 0.52)]
    poly(lip)
    poly([(-0.62, 0.40), (-0.55, 0.40), (-0.55, 0.52), (-0.62, 0.52)])
    poly([(-0.62, -0.52), (-0.55, -0.52), (-0.55, -0.40), (-0.62, -0.40)])
    rear = [(-0.55, -0.52), (-0.35, -0.52), (0.25, -hw(0.25)), (0.25, hw(0.25)), (-0.35, 0.52), (-0.55, 0.52)]
    poly(rear)
    rod((0.22, 0, T + 0.035), (0.62, 0, T + 0.16), 0.06, steel)
    rod((0.6, 0, T + 0.15), (1.95, 0, T + 0.45), 0.15, mat(FALU, 0.32))
    # The slice rises out of the slot and curls back over the blade.
    bm = bmesh.new(); x0, z0, r0, w = -0.585, T + 0.05, 0.2, 0.38; prev = None
    for k in range(121):
        t = k / 120 * 1.7 * math.pi; rr = r0 * (1 - 0.3 * t / math.pi)
        x = x0 + rr * math.sin(t); z = z0 + r0 - rr * math.cos(t)
        a, b = bm.verts.new((x, -w, z)), bm.verts.new((x, w, z))
        if prev: bm.faces.new((prev[0], prev[1], b, a))
        prev = (a, b)
    me = bpy.data.meshes.new("slice"); bm.to_mesh(me); sl = bpy.data.objects.new("slice", me); sc.collection.objects.link(sl)
    sl.modifiers.new("t", "SOLIDIFY").thickness = 0.012; sl.data.materials.append(mat("#FFF8EA", 0.55))
    for p in me.polygons: p.use_smooth = True
    target = Vector((0.25, 0, 0.75)); cam = (-2.6, -5.6, 5.0)
elif concept == "osthyvel3":
    # A Swedish block cheese: yellow cut faces, falu-red wax on the outside.
    T = 0.78
    box((1.75, 1.3, T), (-0.05, 0.05, T / 2), "#F2C661", bevel=0.03, rough=0.6)
    box((0.05, 1.32, T + 0.01), (-0.95, 0.05, T / 2), FALU, bevel=0.02, rough=0.3)
    box((1.8, 0.05, T + 0.01), (-0.05, 0.72, T / 2), FALU, bevel=0.02, rough=0.3); steel = metal("#C4C7CA")
    def hw(x):  # half width of the blade along its length: broad spade, tapering to the neck
        return 0.52 if x <= -0.35 else 0.52 - (x + 0.35) / 0.6 * 0.34
    def poly(pts):
        bm = bmesh.new(); vs = [bm.verts.new((x, y, T + 0.02)) for x, y in pts]; bm.faces.new(vs)
        me = bpy.data.meshes.new("blade"); bm.to_mesh(me); o = bpy.data.objects.new("blade", me); sc.collection.objects.link(o)
        o.modifiers.new("t", "SOLIDIFY").thickness = 0.028; b = o.modifiers.new("b", "BEVEL"); b.width = 0.008; b.segments = 3
        o.data.materials.append(steel); return o
    r = 0.12  # rounded front corners
    front = [(-0.92 + r - r * math.cos(a), sgn * (0.52 - r + r * math.sin(a))) for sgn in (-1,) for a in [i * math.pi / 12 for i in range(7)]]
    lip = [(-0.62, -0.52)] + [(-0.92 + r - r * math.sin(a), -(0.52 - r) - r * math.cos(a)) for a in [i * math.pi / 12 for i in range(7)]][::-1]
    lip = [(-0.62, -0.52), (-0.80, -0.52)] + [(-0.92 + r - r * math.sin(a), -(0.52 - r) - r * math.cos(a)) for a in [i * math.pi / 12 for i in range(1, 7)]] + \
          [(-0.92 + r - r * math.sin(a), (0.52 - r) + r * math.cos(a)) for a in [i * math.pi / 12 for i in range(6, 0, -1)]] + [(-0.80, 0.52), (-0.62, 0.52)]
    poly(lip)
    poly([(-0.62, 0.40), (-0.55, 0.40), (-0.55, 0.52), (-0.62, 0.52)])
    poly([(-0.62, -0.52), (-0.55, -0.52), (-0.55, -0.40), (-0.62, -0.40)])
    rear = [(-0.55, -0.52), (-0.35, -0.52), (0.25, -hw(0.25)), (0.25, hw(0.25)), (-0.35, 0.52), (-0.55, 0.52)]
    poly(rear)
    rod((0.22, 0, T + 0.035), (0.62, 0, T + 0.16), 0.06, steel)
    rod((0.6, 0, T + 0.15), (1.95, 0, T + 0.45), 0.15, mat("#B8865A", 0.55))
    # The slice rises out of the slot and curls back over the blade.
    bm = bmesh.new(); x0, z0, r0, w = -0.585, T + 0.05, 0.34, 0.37; prev = None
    for k in range(121):
        t = k / 120 * 0.95 * math.pi; rr = r0 * (1 - 0.25 * t / math.pi)
        x = x0 + rr * math.sin(t); z = z0 + r0 - rr * math.cos(t)
        a, b = bm.verts.new((x, -w, z)), bm.verts.new((x, w, z))
        if prev: bm.faces.new((prev[0], prev[1], b, a))
        prev = (a, b)
    me = bpy.data.meshes.new("slice"); bm.to_mesh(me); sl = bpy.data.objects.new("slice", me); sc.collection.objects.link(sl)
    sl.modifiers.new("t", "SOLIDIFY").thickness = 0.01
    cm = mat("#F6D27A", 0.45); cb = cm.node_tree.nodes["Principled BSDF"]
    cb.inputs["Subsurface Weight"].default_value = 0.4; cb.inputs["Subsurface Radius"].default_value = (0.2, 0.15, 0.05)
    sl.data.materials.append(cm)
    for p in me.polygons: p.use_smooth = True
    target = Vector((0.35, 0.05, 0.75)); cam = (-2.2, -5.0, 4.4)
elif concept == "osthyvel4":
    # The backlog: envelopes with a few coloured task cards, slightly messy.
    layers = [("#F3ECDD", 3, 0, 0), (MUSTARD, -6, 0.12, -0.05), ("#FBF7EE", 2, 0, 0), ("#EFE6D2", -2, 0, 0),
              (PINE, 5, -0.1, 0.08), ("#FBF7EE", -1, 0, 0), (BLUE, -4, 0.08, 0.1), ("#F3ECDD", 1, 0, 0), ("#FBF7EE", 0, 0, 0)]
    for i, (c, rz, dx, dy) in enumerate(layers):
        box((2.1, 1.5, 0.07), (0.55 + dx, dy, 0.035 + 0.075 * i), c, rot=(0, 0, rz), bevel=0.012, rough=0.7)
    T = 0.075 * len(layers) + 0.005
    # Envelope flap on the top envelope: a shallow pyramid from the far edge, so the stack reads as mail.
    bm = bmesh.new(); z = T - 0.003
    vs = [bm.verts.new(p) for p in ((-0.5, 0.75, z), (1.6, 0.75, z), (0.55, -0.02, z + 0.025))]
    bm.faces.new(vs); me = bpy.data.meshes.new("flap"); bm.to_mesh(me)
    fl = bpy.data.objects.new("flap", me); sc.collection.objects.link(fl)
    fl.modifiers.new("t", "SOLIDIFY").thickness = 0.01; fl.modifiers.new("b", "BEVEL").width = 0.006
    fl.data.materials.append(mat("#F3ECDD", 0.7))
    steel = metal("#CBCDCF"); steel.node_tree.nodes["Principled BSDF"].inputs["Roughness"].default_value = 0.34
    def arc(cx, cy, r, a0, a1, n=10):
        return [(cx + r * math.cos(math.radians(a0 + (a1 - a0) * k / n)), cy + r * math.sin(math.radians(a0 + (a1 - a0) * k / n))) for k in range(n + 1)]
    # Blade outline: rounded tab at the neck, flaring to a broad end with rounded corners.
    outer = arc(-0.17, 0, 0.13, 90, 270) + [(0.0, -0.13), (0.14, -0.22)] + arc(1.17, -0.345, 0.18, 270, 360) + \
            arc(1.17, 0.345, 0.18, 0, 90) + [(0.14, 0.22), (0.0, 0.13)]
    slot = arc(1.0, -0.30, 0.045, 180, 360) + arc(1.0, 0.30, 0.045, 0, 180)
    cu = bpy.data.curves.new("blade", "CURVE"); cu.dimensions = "2D"; cu.fill_mode = "BOTH"
    cu.extrude = 0.012; cu.bevel_depth = 0.004
    for pts in (outer, slot):
        sp = cu.splines.new("POLY"); sp.points.add(len(pts) - 1); sp.use_cyclic_u = True
        for p, (x, y) in zip(sp.points, pts): p.co = (x, y, 0, 1)
    bl = bpy.data.objects.new("blade", cu); sc.collection.objects.link(bl); bl.location = (0, 0, T + 0.016); cu.materials.append(steel)
    # Neck, steel ferrule, wooden handle, end cap: in line with the blade, rising a little off the stack.
    rod((-0.2, 0, T + 0.02), (-0.34, 0, T + 0.06), 0.07, steel)
    rod((-0.3, 0, T + 0.055), (-0.62, 0, T + 0.1), 0.115, steel)
    wood = bpy.data.materials.new("wood"); wood.use_nodes = True; nt = wood.node_tree; b = nt.nodes["Principled BSDF"]
    wv = nt.nodes.new("ShaderNodeTexWave"); wv.inputs["Scale"].default_value = 9; wv.inputs["Distortion"].default_value = 6
    ramp = nt.nodes.new("ShaderNodeValToRGB"); ramp.color_ramp.elements[0].color = hexc("#8E5226"); ramp.color_ramp.elements[1].color = hexc("#B9773F")
    nt.links.new(wv.outputs["Fac"], ramp.inputs["Fac"]); nt.links.new(ramp.outputs["Color"], b.inputs["Base Color"]); b.inputs["Roughness"].default_value = 0.5
    rod((-0.62, 0, T + 0.1), (-1.72, 0, T + 0.27), 0.12, wood)
    rod((-1.72, 0, T + 0.27), (-1.8, 0, T + 0.282), 0.12, steel)
    # One thin sheet comes up through the slot and curls back towards the handle.
    bm = bmesh.new(); x0, z0, r0, w = 1.0, T + 0.03, 0.2, 0.29; prev = None
    for k in range(121):
        t = k / 120 * 1.25 * math.pi; rr = r0 * (1 - 0.28 * t / math.pi)
        x = x0 - rr * math.sin(t); z = z0 + r0 - rr * math.cos(t)
        a, b2 = bm.verts.new((x, -w, z)), bm.verts.new((x, w, z))
        if prev: bm.faces.new((prev[0], prev[1], b2, a))
        prev = (a, b2)
    me = bpy.data.meshes.new("slice"); bm.to_mesh(me); sl = bpy.data.objects.new("slice", me); sc.collection.objects.link(sl)
    sl.modifiers.new("t", "SOLIDIFY").thickness = 0.008; sl.data.materials.append(mat("#FFFBF2", 0.6))
    for p in me.polygons: p.use_smooth = True
    target = Vector((-0.2, 0.05, 0.5)); cam = (-2.2, -4.4, 6.9)
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

bpy.ops.object.camera_add(location=cam); c = bpy.context.object; sc.camera = c; c.data.lens = 62
c.rotation_euler = (target - c.location).to_track_quat("-Z", "Y").to_euler()
sc.render.filepath = out; bpy.ops.render.render(write_still=True)
