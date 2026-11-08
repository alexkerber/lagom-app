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
    # Widest at the slot by the neck, tapering to a rounded tip: the shape of a classic slicer.
    outer = arc(-0.12, 0, 0.09, 90, 270) + [(0.0, -0.09)] + arc(0.22, -0.40, 0.12, 180, 270) + \
            arc(1.21, -0.16, 0.14, 270, 360) + arc(1.21, 0.16, 0.14, 0, 90) + arc(0.22, 0.40, 0.12, 90, 180) + [(0.0, 0.09)]
    slot = arc(0.36, -0.36, 0.035, 180, 360) + arc(0.36, 0.36, 0.035, 0, 180)
    cu = bpy.data.curves.new("blade", "CURVE"); cu.dimensions = "2D"; cu.fill_mode = "BOTH"
    cu.extrude = 0.012; cu.bevel_depth = 0.004
    for pts in (outer, slot):
        sp = cu.splines.new("POLY"); sp.points.add(len(pts) - 1); sp.use_cyclic_u = True
        for p, (x, y) in zip(sp.points, pts): p.co = (x, y, 0, 1)
    bl = bpy.data.objects.new("blade", cu); sc.collection.objects.link(bl); bl.location = (0, 0, T + 0.016); cu.materials.append(steel)
    # Neck, steel ferrule, wooden handle, end cap: in line with the blade, rising a little off the stack.
    rod((-0.14, 0, T + 0.02), (-0.34, 0, T + 0.14), 0.06, steel)
    rod((-0.3, 0, T + 0.12), (-0.58, 0, T + 0.27), 0.1, steel)
    wood = bpy.data.materials.new("wood"); wood.use_nodes = True; nt = wood.node_tree; b = nt.nodes["Principled BSDF"]
    wv = nt.nodes.new("ShaderNodeTexWave"); wv.inputs["Scale"].default_value = 9; wv.inputs["Distortion"].default_value = 6
    ramp = nt.nodes.new("ShaderNodeValToRGB"); ramp.color_ramp.elements[0].color = hexc("#8E5226"); ramp.color_ramp.elements[1].color = hexc("#B9773F")
    nt.links.new(wv.outputs["Fac"], ramp.inputs["Fac"]); nt.links.new(ramp.outputs["Color"], b.inputs["Base Color"]); b.inputs["Roughness"].default_value = 0.5
    rod((-0.58, 0, T + 0.27), (-1.5, 0, T + 0.78), 0.11, wood)
    rod((-1.5, 0, T + 0.78), (-1.57, 0, T + 0.82), 0.11, steel)
    # One thin sheet comes up through the slot, lies on the broad end and curls at the tip.
    z0, w = T + 0.032, 0.27
    path = [(0.34, z0 - 0.03), (0.38, z0 + 0.005)] + [(0.38 + 0.56 * k / 20, z0 + 0.012 + 0.03 * (k / 20) ** 2) for k in range(1, 21)]
    cx, cz, r = 0.94, z0 + 0.042 + 0.13, 0.13
    path += [(cx + r * math.sin(t), cz - r * math.cos(t)) for t in (k / 30 * 1.2 * math.pi for k in range(1, 31))]
    bm = bmesh.new(); prev = None
    for x, z in path:
        a, b2 = bm.verts.new((x, -w, z)), bm.verts.new((x, w, z))
        if prev: bm.faces.new((prev[0], prev[1], b2, a))
        prev = (a, b2)
    me = bpy.data.meshes.new("slice"); bm.to_mesh(me); sl = bpy.data.objects.new("slice", me); sc.collection.objects.link(sl)
    sl.modifiers.new("t", "SOLIDIFY").thickness = 0.008; sl.data.materials.append(mat("#FFFBF2", 0.6))
    for p in me.polygons: p.use_smooth = True
    target = Vector((-0.15, 0.0, 0.5)); cam = (-1.6, -3.0, 8.2)
elif concept == "osthyvel5":
    # Product-photo pass: brushed steel, oiled wood, paper with fibre and translucency, softbox lighting.
    import random, os; random.seed(7)
    ROOT_Z, KEY, RIM, FILL, SAMPLES = 50, 900, 400, 100, int(os.environ.get("SAMPLES", 512))
    TARGET, CAM, LENS, FSTOP = (0.2, 0.15, 0.3), (0.2, -3.4, 9.0), 78, 1.6
    for o in [o for o in sc.objects if o.type == "LIGHT"]: bpy.data.objects.remove(o)
    before = set(sc.objects)
    def node(nt, kind, **kw):
        n = nt.nodes.new(kind)
        for k, v in kw.items(): setattr(n, k, v)
        return n
    def paper(h, text=False, translucent=0.0, rough=0.62):
        m = bpy.data.materials.new("paper" + h); m.use_nodes = True; nt = m.node_tree; L = nt.links
        b = nt.nodes["Principled BSDF"]; b.inputs["Roughness"].default_value = rough
        b.inputs["Sheen Weight"].default_value = 0.25; b.inputs["Specular IOR Level"].default_value = 0.35
        tc = node(nt, "ShaderNodeTexCoord")
        fib = node(nt, "ShaderNodeTexNoise"); fib.inputs["Scale"].default_value = 90; fib.inputs["Detail"].default_value = 12
        fib.inputs["Roughness"].default_value = 0.7; L.new(tc.outputs["Object"], fib.inputs["Vector"])
        bump = node(nt, "ShaderNodeBump"); bump.inputs["Strength"].default_value = 0.2; bump.inputs["Distance"].default_value = 0.002
        L.new(fib.outputs["Fac"], bump.inputs["Height"]); L.new(bump.outputs["Normal"], b.inputs["Normal"])
        mot = node(nt, "ShaderNodeTexNoise"); mot.inputs["Scale"].default_value = 18; mot.inputs["Detail"].default_value = 6; L.new(tc.outputs["Object"], mot.inputs["Vector"])
        tint = node(nt, "ShaderNodeMix", data_type="RGBA", blend_type="MULTIPLY"); tint.inputs["Factor"].default_value = 0.1
        tint.inputs["A"].default_value = hexc(h); L.new(mot.outputs["Color"], tint.inputs["B"]); col = tint.outputs["Result"]
        if text:  # faint grey lines of a letter, in the sheet's UV space (u along, v across)
            uv = node(nt, "ShaderNodeSeparateXYZ"); L.new(tc.outputs["UV"], uv.inputs[0])
            def m2(op, a, bb=None, v=None):
                n = node(nt, "ShaderNodeMath", operation=op); L.new(a, n.inputs[0])
                if bb is not None: L.new(bb, n.inputs[1])
                if v is not None: n.inputs[1].default_value = v
                return n.outputs[0]
            un = m2("MULTIPLY", uv.outputs["X"], v=16); row = m2("FLOOR", un); fr = m2("SUBTRACT", un, row)
            wn = node(nt, "ShaderNodeTexWhiteNoise", noise_dimensions="1D"); L.new(row, wn.inputs["W"])
            ln = m2("ADD", m2("MULTIPLY", wn.outputs["Value"], v=0.45), v=0.42)
            mask = m2("MULTIPLY", m2("GREATER_THAN", fr, v=0.42), m2("LESS_THAN", fr, v=0.58))
            mask = m2("MULTIPLY", mask, m2("GREATER_THAN", uv.outputs["Y"], v=0.16))
            mask = m2("MULTIPLY", mask, m2("LESS_THAN", uv.outputs["Y"], m2("ADD", ln, v=0.16)))
            mask = m2("MULTIPLY", mask, m2("MULTIPLY", m2("GREATER_THAN", uv.outputs["X"], v=0.2), m2("LESS_THAN", uv.outputs["X"], v=0.86)))
            wn2 = node(nt, "ShaderNodeTexWhiteNoise", noise_dimensions="1D"); L.new(m2("ADD", row, v=13.0), wn2.inputs["W"])
            mask = m2("MULTIPLY", mask, m2("GREATER_THAN", wn2.outputs["Value"], v=0.18))
            wd = node(nt, "ShaderNodeTexWhiteNoise", noise_dimensions="1D")
            L.new(m2("ADD", m2("FLOOR", m2("MULTIPLY", uv.outputs["Y"], v=14)), m2("MULTIPLY", row, v=3.7)), wd.inputs["W"])
            mask = m2("MULTIPLY", mask, m2("GREATER_THAN", wd.outputs["Value"], v=0.16))
            ink = node(nt, "ShaderNodeMix", data_type="RGBA"); L.new(m2("MULTIPLY", mask, v=0.38), ink.inputs["Factor"])
            L.new(col, ink.inputs["A"]); ink.inputs["B"].default_value = hexc("#6E6A63"); col = ink.outputs["Result"]
        L.new(col, b.inputs["Base Color"])
        if translucent:
            tr = node(nt, "ShaderNodeBsdfTranslucent"); L.new(col, tr.inputs["Color"])
            mx = node(nt, "ShaderNodeMixShader"); mx.inputs[0].default_value = translucent
            out = nt.nodes["Material Output"]; L.new(b.outputs[0], mx.inputs[1]); L.new(tr.outputs[0], mx.inputs[2]); L.new(mx.outputs[0], out.inputs["Surface"])
        return m
    def steel5(radial):
        # Brushed stainless: anisotropic along the blade, or around the axis on turned parts.
        m = bpy.data.materials.new("steel"); m.use_nodes = True; nt = m.node_tree; L = nt.links; b = nt.nodes["Principled BSDF"]
        b.inputs["Base Color"].default_value = hexc("#CED0D2"); b.inputs["Metallic"].default_value = 1.0
        b.inputs["Roughness"].default_value = 0.2 if radial else 0.24; b.inputs["Anisotropic"].default_value = 0.8
        if radial:
            tg = node(nt, "ShaderNodeTangent", direction_type="RADIAL", axis="Z"); L.new(tg.outputs[0], b.inputs["Tangent"])
        else:
            vt = node(nt, "ShaderNodeVectorTransform", vector_type="VECTOR", convert_from="OBJECT", convert_to="WORLD")
            vt.inputs[0].default_value = (1, 0, 0); L.new(vt.outputs[0], b.inputs["Tangent"])
        tc = node(nt, "ShaderNodeTexCoord"); mp = node(nt, "ShaderNodeMapping")
        mp.inputs["Scale"].default_value = (1, 1, 900) if radial else (2, 900, 900); L.new(tc.outputs["Object"], mp.inputs[0])
        if radial:  # turned parts: lines around the axis, so noise varies along Z only
            mp.inputs["Scale"].default_value = (0, 0, 900)
        nz = node(nt, "ShaderNodeTexNoise"); nz.inputs["Scale"].default_value = 1; nz.inputs["Detail"].default_value = 3; L.new(mp.outputs[0], nz.inputs["Vector"])
        bump = node(nt, "ShaderNodeBump"); bump.inputs["Strength"].default_value = 0.08; bump.inputs["Distance"].default_value = 0.0004
        L.new(nz.outputs["Fac"], bump.inputs["Height"]); L.new(bump.outputs["Normal"], b.inputs["Normal"])
        sm = node(nt, "ShaderNodeTexNoise"); sm.inputs["Scale"].default_value = 4; L.new(tc.outputs["Object"], sm.inputs["Vector"])
        rr = node(nt, "ShaderNodeMapRange"); rr.inputs["To Min"].default_value = b.inputs["Roughness"].default_value - 0.02
        rr.inputs["To Max"].default_value = b.inputs["Roughness"].default_value + 0.02; L.new(sm.outputs["Fac"], rr.inputs[0]); L.new(rr.outputs[0], b.inputs["Roughness"])
        return m
    def oak():
        m = bpy.data.materials.new("wood"); m.use_nodes = True; nt = m.node_tree; L = nt.links; b = nt.nodes["Principled BSDF"]
        tc = node(nt, "ShaderNodeTexCoord"); mp = node(nt, "ShaderNodeMapping"); mp.inputs["Location"].default_value = (0.31, 0.17, 0)
        L.new(tc.outputs["Object"], mp.inputs[0])
        mp.inputs["Scale"].default_value = (1, 1, 0.06)
        wv = node(nt, "ShaderNodeTexWave", wave_type="BANDS", bands_direction="X"); wv.inputs["Scale"].default_value = 16
        wv.inputs["Distortion"].default_value = 9; wv.inputs["Detail"].default_value = 8; wv.inputs["Detail Scale"].default_value = 1.6; wv.inputs["Detail Roughness"].default_value = 0.7
        L.new(mp.outputs[0], wv.inputs["Vector"])
        ramp = node(nt, "ShaderNodeValToRGB"); e = ramp.color_ramp.elements
        e[0].position, e[0].color = 0.5, hexc("#96643C"); e[1].position, e[1].color = 1.0, hexc("#5A3A22")
        L.new(wv.outputs["Fac"], ramp.inputs[0])
        pm = node(nt, "ShaderNodeMapping"); pm.inputs["Scale"].default_value = (140, 140, 2); L.new(tc.outputs["Object"], pm.inputs[0])
        pore = node(nt, "ShaderNodeTexNoise"); pore.inputs["Scale"].default_value = 1; pore.inputs["Detail"].default_value = 6; L.new(pm.outputs[0], pore.inputs["Vector"])
        mx = node(nt, "ShaderNodeMix", data_type="RGBA", blend_type="MULTIPLY"); mx.inputs["Factor"].default_value = 0.5
        L.new(ramp.outputs[0], mx.inputs["A"]); L.new(pore.outputs["Color"], mx.inputs["B"]); L.new(mx.outputs["Result"], b.inputs["Base Color"])
        b.inputs["Roughness"].default_value = 0.5; b.inputs["Coat Weight"].default_value = 0.15; b.inputs["Coat Roughness"].default_value = 0.3
        bump = node(nt, "ShaderNodeBump"); bump.inputs["Strength"].default_value = 0.08; bump.inputs["Distance"].default_value = 0.002
        L.new(pore.outputs["Fac"], bump.inputs["Height"]); L.new(bump.outputs["Normal"], b.inputs["Normal"])
        return m
    def chaikin(pts, n=4):
        for _ in range(n):
            q = []
            for i in range(len(pts)):
                (x0, y0), (x1, y1) = pts[i], pts[(i + 1) % len(pts)]
                q += [(0.75 * x0 + 0.25 * x1, 0.75 * y0 + 0.25 * y1), (0.25 * x0 + 0.75 * x1, 0.25 * y0 + 0.75 * y1)]
            pts = q
        return pts
    def lathe(prof, p0, d, m, seg=72):
        # Turned part: profile (s along axis, radius), local Z is the axis.
        bm = bmesh.new(); rings = []
        for s, r in prof:
            rings.append([bm.verts.new((0, 0, s))] if r == 0 else
                         [bm.verts.new((r * math.cos(2 * math.pi * k / seg), r * math.sin(2 * math.pi * k / seg), s)) for k in range(seg)])
        for a, b2 in zip(rings, rings[1:]):
            for k in range(seg):
                if len(a) == 1: bm.faces.new((a[0], b2[k], b2[(k + 1) % seg]))
                elif len(b2) == 1: bm.faces.new((a[k], b2[0], a[(k + 1) % seg])[::-1])
                else: bm.faces.new((a[k], a[(k + 1) % seg], b2[(k + 1) % seg], b2[k]))
        bmesh.ops.recalc_face_normals(bm, faces=bm.faces)
        me = bpy.data.meshes.new("lathe"); bm.to_mesh(me); o = bpy.data.objects.new("lathe", me); sc.collection.objects.link(o)
        for p in me.polygons: p.use_smooth = True
        o.location = p0; o.rotation_euler = Vector(d).to_track_quat("Z", "Y").to_euler(); o.data.materials.append(m); return o

    # The backlog: envelopes with a few task cards between them, slightly messy.
    env_c = ["#EFE8DA", "#F6F2EA", "#C9A57A", "#F8F5EE"]; cards = [("#2F56A8", -9, 0.62, -0.3), ("#1E6B4E", 13, -0.45, 0.42), ("#D9A441", -4, 0.3, -0.62)]
    z = 0.0; cx0 = 0.55
    def sheet(w, d, loc, rz, th, m, amp):
        # Paper is never quite flat: flat in the middle where things rest, edges and corners lift a little.
        bpy.ops.mesh.primitive_grid_add(x_subdivisions=48, y_subdivisions=36, size=1, location=loc, rotation=(0, 0, math.radians(rz)))
        o = bpy.context.object; ph = [random.uniform(0, 6) for _ in range(4)]
        for v in o.data.vertices:
            x, y = v.co.x * 2, v.co.y * 2; e = max(abs(x), abs(y)) ** 3
            v.co.x *= w; v.co.y *= d
            v.co.z = amp * e * (0.6 + 0.5 * math.sin(2.3 * x + ph[0]) * math.sin(1.9 * y + ph[1]) + 0.4 * math.sin(5 * x * y + ph[2]))
        sd = o.modifiers.new("t", "SOLIDIFY"); sd.thickness = th; sd.offset = -1
        bv = o.modifiers.new("b", "BEVEL"); bv.width = th * 0.45; bv.segments = 4; bv.limit_method = "ANGLE"
        o.data.materials.append(m); bpy.ops.object.shade_smooth(); return o
    for i in range(4):
        top = sheet(2.15, 1.5, (cx0 + random.uniform(-0.07, 0.07), random.uniform(-0.06, 0.06), z + 0.03), random.uniform(-6, 6) if i < 3 else 1.5,
              0.03, paper(env_c[i]), 0.012 if i < 3 else 0.006); z += 0.032
        if i < 3:
            c, rz, dx, dy = cards[i]
            sheet(1.55, 0.95, (cx0 + dx, dy, z + 0.014), rz, 0.014, paper(c, rough=0.55), 0.02); z += 0.016
    T = z
    # Back of the top envelope: bottom flap rising to the middle, V flap over it, each a thin layer.
    for k, (pts, dz) in enumerate((([(-1.06, -0.73), (1.06, -0.73), (0.2, 0.06), (-0.2, 0.06)], 0.003),
                                   ([(-1.06, 0.73), (1.06, 0.73), (0.08, -0.04), (-0.08, -0.04)], 0.007))):
        fl_pts = chaikin([(a[0] + (b[0] - a[0]) * f / 6, a[1] + (b[1] - a[1]) * f / 6) for a, b in zip(pts, pts[1:] + pts[:1]) for f in range(6)], 3)
        cu = bpy.data.curves.new("flap", "CURVE"); cu.dimensions = "2D"; cu.fill_mode = "BOTH"; cu.extrude = 0.002; cu.bevel_depth = 0.003
        sp = cu.splines.new("POLY"); sp.points.add(len(fl_pts) - 1); sp.use_cyclic_u = True
        for p, (x, y) in zip(sp.points, fl_pts): p.co = (x, y, 0, 1)
        fl = bpy.data.objects.new("flap", cu); sc.collection.objects.link(fl)
        fl.location = (top.location.x, top.location.y, T + dz); fl.rotation_euler.z = top.rotation_euler.z
        cu.materials.append(paper("#F5F0E6" if k else "#F1EBDF"))
    T += 0.01

    # Blade: widest at the shoulders by the neck, long slot across, tapering to a rounded tip.
    blade_m, turned = steel5(False), steel5(True)
    half = [(-0.14, 0.05), (-0.03, 0.07), (0.03, 0.30), (0.08, 0.47), (0.17, 0.51), (0.40, 0.48), (1.18, 0.22), (1.36, 0.14), (1.43, 0.05)]
    outer = chaikin(half + [(x, -y) for x, y in half[::-1]], 4)
    sx = 0.29; slot = chaikin([(sx - 0.025, -0.41), (sx + 0.025, -0.41), (sx + 0.025, 0.41), (sx - 0.025, 0.41)], 4)
    cu = bpy.data.curves.new("blade", "CURVE"); cu.dimensions = "2D"; cu.fill_mode = "BOTH"; cu.extrude = 0.004; cu.bevel_depth = 0.004; cu.bevel_resolution = 3
    for pts in (outer, slot):
        sp = cu.splines.new("POLY"); sp.points.add(len(pts) - 1); sp.use_cyclic_u = True
        for p, (x, y) in zip(sp.points, pts): p.co = (x, y, 0, 1)
    bl = bpy.data.objects.new("blade", cu); sc.collection.objects.link(bl); bl.location = (0, 0, T + 0.0085); cu.materials.append(blade_m)
    Tb = T + 0.017
    # Neck: a flat steel strip from the blade tab bending up into the handle axis.
    ang = math.radians(24); ax = Vector((-math.cos(ang), 0, math.sin(ang)))
    h0 = Vector((-0.42, 0, Tb + 0.105))
    sec = bpy.data.curves.new("sec", "CURVE"); sec.dimensions = "2D"
    pts = chaikin([(-0.06, -0.011), (0.06, -0.011), (0.06, 0.011), (-0.06, 0.011)], 3)
    sp = sec.splines.new("POLY"); sp.points.add(len(pts) - 1); sp.use_cyclic_u = True
    for p, (x, y) in zip(sp.points, pts): p.co = (x, y, 0, 1)
    so = bpy.data.objects.new("sec", sec); sc.collection.objects.link(so); so.hide_render = True
    nk = bpy.data.curves.new("neck", "CURVE"); nk.dimensions = "3D"; nk.bevel_mode = "OBJECT"; nk.bevel_object = so; nk.use_fill_caps = True
    nk.twist_mode = "Z_UP"
    sp = nk.splines.new("BEZIER"); path = [(-0.06, 0, Tb - 0.008), (-0.2, 0, Tb + 0.012), h0 + ax * 0.06]
    sp.bezier_points.add(len(path) - 1)
    for p, co in zip(sp.bezier_points, path):
        p.co = co; p.handle_left_type = p.handle_right_type = "AUTO"
    sp.bezier_points[0].radius = 0.9; sp.bezier_points[-1].radius = 0.75
    no = bpy.data.objects.new("neck", nk); sc.collection.objects.link(no); nk.materials.append(blade_m)
    # Turned steel bolster, oiled oak grip, steel end cap.
    lathe([(0, 0), (0, 0.045), (0.03, 0.052), (0.2, 0.088), (0.235, 0.094), (0.25, 0.092), (0.25, 0)], h0, ax, turned)
    g = [(0, 0), (0, 0.092)] + [(0.02 + 0.9 * k / 12, 0.094 + 0.014 * math.sin(math.pi * k / 12)) for k in range(13)] + [(0.94, 0.092), (0.94, 0)]
    lathe(g, h0 + ax * 0.25, ax, oak())
    lathe([(0, 0), (0, 0.092), (0.035, 0.092), (0.06, 0.083), (0.076, 0.062), (0.085, 0.034), (0.088, 0)], h0 + ax * 1.19, ax, turned)

    # The slice: one sheet of mail, up through the slot, lying on the blade, lifting at the free end.
    pp = [(sx - 0.09, T - 0.006), (sx - 0.04, T - 0.004), (sx + 0.015, Tb - 0.004), (sx + 0.045, Tb + 0.003), (sx + 0.07, Tb + 0.0035)]
    pp += [(sx + 0.07 + 0.52 * k / 20, Tb + 0.0035 + 0.003 * math.sin(math.pi * k / 20) ** 2) for k in range(1, 21)]
    R, x1 = 0.24, sx + 0.57
    pp += [(x1 + R * math.sin(a), Tb + 0.0035 + R - R * math.cos(a)) for a in (math.radians(34 * k / 16) for k in range(1, 17))]
    acc = [0.0]
    for a, b2 in zip(pp, pp[1:]): acc.append(acc[-1] + math.dist(a, b2))
    Lp = acc[-1]
    def at(s):
        for i in range(1, len(acc)):
            if acc[i] >= s:
                f = (s - acc[i - 1]) / (acc[i] - acc[i - 1] or 1); (xa, za), (xb, zb) = pp[i - 1], pp[i]
                return xa + f * (xb - xa), za + f * (zb - za)
        return pp[-1]
    M, N = 30, 160; tear = [random.uniform(0, 0.025) for _ in range(M + 1)]
    ragged = [random.uniform(-0.004, 0.004) for _ in range(N + 1)]; ph = [random.uniform(0, 6) for _ in range(6)]
    bm = bmesh.new(); uvl = bm.loops.layers.uv.new("UVMap"); grid = []
    for j in range(M + 1):
        t = j / M * 2 - 1; smax = Lp - 0.012 - 0.03 * (0.5 + 0.5 * math.sin(7.3 * t + 1)) * (0.6 + tear[j] * 20) * 0.5
        col = []
        for k in range(N + 1):
            s = k / N * smax; x, zz = at(s)
            sg = 0 if t < 0 else 3
            hw = 0.33 - 0.14 * s / Lp + 0.05 * max(0, 1 - s / 0.15) + 0.005 * math.sin(s * 17 + ph[sg]) + 0.004 * math.sin(s * 41 + ph[sg + 1]) + 0.002 * math.sin(s * 97 + ph[sg + 2]) + (ragged[k] if abs(t) == 1 else 0)
            col.append((bm.verts.new((x, t * hw, zz)), s / Lp, (t + 1) / 2))
        grid.append(col)
    for j in range(M):
        for k in range(N):
            vs = (grid[j][k], grid[j + 1][k], grid[j + 1][k + 1], grid[j][k + 1])
            f = bm.faces.new([v[0] for v in vs])
            for lp, v in zip(f.loops, vs): lp[uvl].uv = (v[1], v[2])
    me = bpy.data.meshes.new("slice"); bm.to_mesh(me); sl = bpy.data.objects.new("slice", me); sc.collection.objects.link(sl)
    for p in me.polygons: p.use_smooth = True
    sd = sl.modifiers.new("t", "SOLIDIFY"); sd.thickness = 0.004; sd.offset = 0
    sl.data.materials.append(paper("#FCFAF5", text=True, translucent=0.28, rough=0.55))

    # Everything on one root, turned so the slicer runs diagonally in the frame.
    root = bpy.data.objects.new("root", None); sc.collection.objects.link(root)
    for o in set(sc.objects) - before:
        if o is not root and o.parent is None: o.parent = root
    root.rotation_euler.z = math.radians(ROOT_Z)
    # Studio: big softbox key, strip rim, soft fill, and a dim gradient tent for the steel to reflect.
    def area(loc, energy, sx_, sy_, target=(0.3, 0, 0.3)):
        bpy.ops.object.light_add(type="AREA", location=loc); l = bpy.context.object
        l.data.shape = "RECTANGLE"; l.data.size, l.data.size_y = sx_, sy_; l.data.energy = energy
        l.rotation_euler = (Vector(target) - Vector(loc)).to_track_quat("-Z", "Y").to_euler(); return l
    area((-1.5, -2.2, 7.0), KEY, 4, 4); area((4.0, 3.5, 2.2), RIM, 1.2, 6).data.use_shadow = False; area((0.5, -6, 3.5), FILL, 6, 3).visible_glossy = False  # fill the paper, keep the steel dark
    wn = w.node_tree; L = wn.links; bg = wn.nodes["Background"]; bg.inputs[1].default_value = 1.0
    tc = wn.nodes.new("ShaderNodeTexCoord"); se = wn.nodes.new("ShaderNodeSeparateXYZ"); L.new(tc.outputs["Generated"], se.inputs[0])
    rp = wn.nodes.new("ShaderNodeValToRGB"); e = rp.color_ramp.elements
    e[0].position, e[0].color = 0.45, (0.015, 0.015, 0.016, 1); e[1].position, e[1].color = 1.0, (0.07, 0.068, 0.066, 1)
    mr = wn.nodes.new("ShaderNodeMapRange"); mr.inputs["From Min"].default_value = -1; L.new(se.outputs["Z"], mr.inputs[0])
    L.new(mr.outputs[0], rp.inputs[0]); L.new(rp.outputs[0], bg.inputs[0])
    sc.cycles.samples = SAMPLES; sc.cycles.adaptive_threshold = 0.008; sc.cycles.denoiser = "OPENIMAGEDENOISE"
    sc.cycles.blur_glossy = 0.5; sc.cycles.sample_clamp_indirect = 6
    pr = bpy.context.preferences.addons["cycles"].preferences; pr.compute_device_type = "METAL"; pr.get_devices()
    for d in pr.devices: d.use = d.type == "METAL"
    sc.cycles.device = "GPU"
    target = Vector(TARGET); cam = CAM
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
if concept == "osthyvel5":
    c.data.lens = LENS; c.data.shift_x = 0.026; c.data.dof.use_dof = True; c.data.dof.aperture_fstop = FSTOP
    c.data.dof.focus_distance = (target - c.location).length
sc.render.filepath = out; bpy.ops.render.render(write_still=True)

if concept == "osthyvel5":
    # Light and dark plates: an 824 px rounded square with a soft drop shadow, the render on top.
    import numpy as np
    src = bpy.data.images.load(out); W = src.size[0]
    fg = np.array(src.pixels[:], dtype=np.float32).reshape(W, W, 4)  # rows run bottom to top
    a = fg[..., 3:4]
    yy, xx = np.mgrid[0:W, 0:W].astype(np.float32) + 0.5
    def sdf(cx, cy, half, r):
        qx, qy = np.abs(xx - cx) - half + r, np.abs(yy - cy) - half + r
        return np.hypot(np.maximum(qx, 0), np.maximum(qy, 0)) + np.minimum(np.maximum(qx, qy), 0) - r
    d = sdf(W / 2, W / 2, 412, 185); plate = np.clip(0.5 - d, 0, 1)[..., None]
    sh = (0.28 * np.clip(1 - (sdf(W / 2, W / 2 - 10, 412, 185) + 2) / 26, 0, 1) ** 2)[..., None]
    t = (yy / W)[..., None]  # 0 at the bottom
    for name, top, bot in (("light", "#F8F3EA", "#E8E0D2"), ("dark", "#272727", "#191919")):
        g = lambda h: np.array([int(h[i:i + 2], 16) / 255 for i in (1, 3, 5)], dtype=np.float32)
        base = g(bot) + (g(top) - g(bot)) * t
        rgb = fg[..., :3] * a + base * (1 - a)
        alpha = plate + sh * (1 - plate)
        rgb = rgb * plate / np.maximum(alpha, 1e-6)  # outside the plate only the drop shadow (black) remains
        o = bpy.data.images.new(name, W, W, alpha=True)
        o.pixels[:] = np.concatenate([rgb, alpha], axis=2).ravel()
        p = out.replace(".png", f"-{name}.png"); o.filepath_raw = p; o.file_format = "PNG"; o.save()
