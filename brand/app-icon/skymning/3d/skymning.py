"""Skymning app icon: frosted-glass envelope on a velvet plate, rendered in Cycles.

Run headless:
  Blender -b -P skymning.py -- icon [res] [samples]   -> icon-<res>.png
  Blender -b -P skymning.py -- layers [res] [samples] -> layer-bg.png, layer-envelope.png
  Blender -b -P skymning.py -- all                    -> both, 1024 px

Scene units: 1 unit = 100 px of the 1024 canvas. The plate is the 824 px macOS
rounded square (-4.12..4.12), the camera looks down -Z, front faces point +Z.
"""
import math
import os
import sys

import bpy
import numpy as np
from mathutils import Vector

HERE = os.path.dirname(os.path.abspath(__file__))
argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
MODE = argv[0] if argv else "icon"
RES = int(argv[1]) if len(argv) > 1 else 1024
SAMPLES = int(argv[2]) if len(argv) > 2 else 512

PLATE = 4.12


def lin(hexstr, a=1.0):
    """sRGB hex -> linear RGBA, because shader colours are scene-linear."""
    c = [int(hexstr[i:i + 2], 16) / 255 for i in (1, 3, 5)]
    return tuple(v / 12.92 if v <= 0.04045 else ((v + 0.055) / 1.055) ** 2.4 for v in c) + (a,)


# ---------------------------------------------------------------- geometry

def inset(poly, d):
    """Offset a convex CCW polygon inward by d."""
    n = len(poly)
    lines = []
    for i in range(n):
        p, q = Vector(poly[i]), Vector(poly[(i + 1) % n])
        e = (q - p).normalized()
        lines.append((p + Vector((-e.y, e.x)) * d, e))
    out = []
    for i in range(n):
        (p1, e1), (p2, e2) = lines[i - 1], lines[i]
        den = e1.x * e2.y - e1.y * e2.x
        t = ((p2.x - p1.x) * e2.y - (p2.y - p1.y) * e2.x) / den
        out.append(p1 + e1 * t)
    return out


def rounded(poly, radii, seg=24):
    """Replace each corner of a convex CCW polygon with a circular arc."""
    n = len(poly)
    pts = []
    for i in range(n):
        a, b, c = Vector(poly[i - 1]), Vector(poly[i]), Vector(poly[(i + 1) % n])
        u, v = (a - b).normalized(), (c - b).normalized()
        half = u.angle(v) / 2
        r = radii[i]
        dist = r / math.tan(half)
        t1 = b + u * dist
        centre = b + (u + v).normalized() * (r / math.sin(half))
        a1 = math.atan2(t1.y - centre.y, t1.x - centre.x)
        t2 = b + v * dist
        a2 = math.atan2(t2.y - centre.y, t2.x - centre.x)
        da = (a2 - a1 + math.pi) % (2 * math.pi) - math.pi
        for k in range(seg + 1):
            ang = a1 + da * k / seg
            pts.append(Vector((centre.x + r * math.cos(ang), centre.y + r * math.sin(ang))))
    return pts


def slab(name, poly, radii, z_back, thick, bev, mat):
    """Extruded rounded polygon with round bevels; outline = outer silhouette."""
    pts = rounded(inset(poly, bev), [max(r - bev, 0.01) for r in radii])
    cu = bpy.data.curves.new(name, "CURVE")
    cu.dimensions = "2D"
    cu.fill_mode = "BOTH"
    cu.extrude = max(thick / 2 - bev, 0.0)
    cu.bevel_depth = bev
    cu.bevel_resolution = 10
    sp = cu.splines.new("POLY")
    sp.points.add(len(pts) - 1)
    for i, p in enumerate(pts):
        sp.points[i].co = (p.x, p.y, 0, 1)
    sp.use_cyclic_u = True
    sp.use_smooth = True
    ob = bpy.data.objects.new(name, cu)
    ob.location.z = z_back + thick / 2
    cu.materials.append(mat)
    bpy.context.scene.collection.objects.link(ob)
    return ob


# ---------------------------------------------------------------- materials

def node_mat(name):
    m = bpy.data.materials.new(name)
    m.use_nodes = True
    nt = m.node_tree
    nt.nodes.clear()
    out = nt.nodes.new("ShaderNodeOutputMaterial")
    return m, nt, out


def plate_material(span):
    """Velvet plate: vertical gradient over +-span, high roughness, sheen, fine grain."""
    m, nt, out = node_mat("plate")
    n, l = nt.nodes, nt.links
    tc = n.new("ShaderNodeTexCoord")
    sep = n.new("ShaderNodeSeparateXYZ")
    l.new(tc.outputs["Object"], sep.inputs[0])
    mr = n.new("ShaderNodeMapRange")
    mr.inputs["From Min"].default_value = -span
    mr.inputs["From Max"].default_value = span
    l.new(sep.outputs["Y"], mr.inputs["Value"])
    ramp = n.new("ShaderNodeValToRGB")
    el = ramp.color_ramp.elements
    stops = [(0.0, "#8A60AA"), (0.25, "#6C589E"), (0.5, "#49528B"), (0.70, "#38537F"), (1.0, "#2A4068")]
    el[0].position, el[0].color = stops[0][0], lin(stops[0][1])
    el[1].position, el[1].color = stops[-1][0], lin(stops[-1][1])
    for pos, col in stops[1:-1]:
        e = el.new(pos)
        e.color = lin(col)
    l.new(mr.outputs["Result"], ramp.inputs["Fac"])

    bsdf = n.new("ShaderNodeBsdfPrincipled")
    l.new(ramp.outputs["Color"], bsdf.inputs["Base Color"])
    bsdf.inputs["Roughness"].default_value = 0.92
    bsdf.inputs["Specular IOR Level"].default_value = 0.15
    bsdf.inputs["Sheen Weight"].default_value = 1.0
    bsdf.inputs["Sheen Roughness"].default_value = 0.45
    bsdf.inputs["Sheen Tint"].default_value = lin("#DCD6FF")

    # velvety grain: tiny noise bump
    noise = n.new("ShaderNodeTexNoise")
    noise.inputs["Scale"].default_value = 600.0
    noise.inputs["Detail"].default_value = 2.0
    l.new(tc.outputs["Object"], noise.inputs["Vector"])
    bump = n.new("ShaderNodeBump")
    bump.inputs["Strength"].default_value = 0.35
    bump.inputs["Distance"].default_value = 0.004
    l.new(noise.outputs["Fac"], bump.inputs["Height"])
    l.new(bump.outputs["Normal"], bsdf.inputs["Normal"])
    l.new(bsdf.outputs[0], out.inputs["Surface"])
    return m


def glass_material():
    """Frosted lavender glass: rough refraction mixed with diffuse translucency."""
    m, nt, out = node_mat("glass")
    n, l = nt.nodes, nt.links
    p = n.new("ShaderNodeBsdfPrincipled")
    p.inputs["Base Color"].default_value = lin("#ECE8F6")
    p.inputs["Transmission Weight"].default_value = 1.0
    p.inputs["Roughness"].default_value = 0.45
    p.inputs["IOR"].default_value = 1.33
    p.inputs["Specular IOR Level"].default_value = 0.6
    tr = n.new("ShaderNodeBsdfTranslucent")
    tr.inputs["Color"].default_value = lin("#E6DEEC")
    df = n.new("ShaderNodeBsdfDiffuse")
    df.inputs["Color"].default_value = lin("#EEE8F4")
    mix1 = n.new("ShaderNodeMixShader")
    mix1.inputs["Fac"].default_value = 0.25
    l.new(p.outputs[0], mix1.inputs[1])
    l.new(tr.outputs[0], mix1.inputs[2])
    # thicker optical path at the bevels scatters more: milkier, whiter rims
    lw = n.new("ShaderNodeLayerWeight")
    lw.inputs["Blend"].default_value = 0.35
    rim = n.new("ShaderNodeMapRange")
    rim.inputs["To Min"].default_value = 0.08
    rim.inputs["To Max"].default_value = 0.9
    l.new(lw.outputs["Facing"], rim.inputs["Value"])
    mix2 = n.new("ShaderNodeMixShader")
    l.new(rim.outputs["Result"], mix2.inputs["Fac"])
    l.new(mix1.outputs[0], mix2.inputs[1])
    l.new(df.outputs[0], mix2.inputs[2])
    l.new(mix2.outputs[0], out.inputs["Surface"])
    return m


def glow_material(strength, radius):
    """Soft disc of warm light: emission fades with distance from the disc centre."""
    m, nt, out = node_mat("glow")
    n, l = nt.nodes, nt.links
    tc = n.new("ShaderNodeTexCoord")
    vm = n.new("ShaderNodeVectorMath")
    vm.operation = "LENGTH"
    l.new(tc.outputs["Object"], vm.inputs[0])
    inv = n.new("ShaderNodeMapRange")
    inv.inputs["From Min"].default_value = 0.0
    inv.inputs["From Max"].default_value = radius
    inv.inputs["To Min"].default_value = 1.0
    inv.inputs["To Max"].default_value = 0.0
    l.new(vm.outputs["Value"], inv.inputs["Value"])
    pw = n.new("ShaderNodeMath")
    pw.operation = "POWER"
    pw.inputs[1].default_value = 1.6
    l.new(inv.outputs["Result"], pw.inputs[0])
    col = n.new("ShaderNodeValToRGB")
    col.color_ramp.elements[0].color = lin("#FF7A34")
    col.color_ramp.elements[1].color = lin("#FFC6A6")
    l.new(pw.outputs[0], col.inputs["Fac"])
    em = n.new("ShaderNodeEmission")
    em.inputs["Strength"].default_value = strength
    l.new(col.outputs["Color"], em.inputs["Color"])
    tp = n.new("ShaderNodeBsdfTransparent")
    mix = n.new("ShaderNodeMixShader")
    l.new(pw.outputs[0], mix.inputs["Fac"])
    l.new(tp.outputs[0], mix.inputs[1])
    l.new(em.outputs[0], mix.inputs[2])
    l.new(mix.outputs[0], out.inputs["Surface"])
    return m


def emission_material(name, hexstr):
    m, nt, out = node_mat(name)
    em = nt.nodes.new("ShaderNodeEmission")
    em.inputs["Color"].default_value = lin(hexstr)
    nt.links.new(em.outputs[0], out.inputs["Surface"])
    return m


# ---------------------------------------------------------------- scene

def reset():
    bpy.ops.wm.read_factory_settings(use_empty=True)
    sc = bpy.context.scene
    sc.render.engine = "CYCLES"
    try:
        prefs = bpy.context.preferences.addons["cycles"].preferences
        prefs.compute_device_type = "METAL"
        prefs.get_devices()
        for d in prefs.devices:
            d.use = True
            print("DEVICE", d.type, d.name)
        sc.cycles.device = "GPU"
    except Exception as exc:  # CPU still works, only slower
        print("GPU unavailable:", exc)
    sc.cycles.samples = SAMPLES
    sc.cycles.use_denoising = True
    sc.cycles.denoiser = "OPENIMAGEDENOISE"
    sc.cycles.max_bounces = 24
    sc.cycles.transmission_bounces = 24
    sc.cycles.transparent_max_bounces = 24
    sc.cycles.diffuse_bounces = 4
    sc.cycles.glossy_bounces = 6
    sc.cycles.blur_glossy = 1.0
    sc.render.resolution_x = sc.render.resolution_y = RES
    sc.render.film_transparent = True
    sc.view_settings.view_transform = "Standard"
    sc.view_settings.look = "None"
    sc.view_settings.exposure = -0.28
    sc.render.image_settings.file_format = "PNG"
    sc.render.image_settings.color_mode = "RGBA"
    sc.render.image_settings.color_depth = "8"

    world = bpy.data.worlds.new("studio")
    world.use_nodes = True
    bg = world.node_tree.nodes["Background"]
    bg.inputs["Color"].default_value = lin("#D8DAE6")
    bg.inputs["Strength"].default_value = 0.8
    sc.world = world

    cam_data = bpy.data.cameras.new("cam")
    cam_data.type = "ORTHO"
    cam_data.ortho_scale = 10.24
    cam = bpy.data.objects.new("cam", cam_data)
    cam.location = (0, 0, 30)
    sc.collection.objects.link(cam)
    sc.camera = cam

    # soft key from above-front; it sets the contact shadow under the envelope
    key = bpy.data.lights.new("key", "AREA")
    key.shape = "DISK"
    key.size = 5.0
    key.energy = 3300
    key.color = lin("#FFFDF8")[:3]
    ko = bpy.data.objects.new("key", key)
    ko.location = (0, 9.5, 11.0)
    ko.rotation_euler = (Vector((0, 0, 0)) - ko.location).to_track_quat("-Z", "Y").to_euler()
    sc.collection.objects.link(ko)
    # broad cool fill from the front so nothing goes dark
    fill = bpy.data.lights.new("fill", "AREA")
    fill.size = 16.0
    fill.energy = 550
    fill.color = lin("#EEF0FF")[:3]
    fo = bpy.data.objects.new("fill", fill)
    fo.location = (0, -2.0, 16.0)
    fo.rotation_euler = (Vector((0, 0, 0)) - fo.location).to_track_quat("-Z", "Y").to_euler()
    sc.collection.objects.link(fo)
    return sc


def build(sc):
    objs = {}
    p = PLATE
    objs["plate"] = slab("plate", [(-p, -p), (p, -p), (p, p), (-p, p)], [1.85] * 4,
                         -0.75, 0.75, 0.34, plate_material(p))
    glass = glass_material()
    # body, lower flaps, top flap: separate slabs stacked toward the camera
    objs["body"] = slab("body", [(-2.90, -2.10), (2.90, -2.10), (2.90, 1.72), (-2.90, 1.72)],
                        [0.40] * 4, 0.60, 0.24, 0.09, glass)
    # vertices sit outside the visible shape: the corner arcs pull the silhouette in
    objs["lower"] = slab("lower", [(-3.35, -2.10), (3.35, -2.10), (0.0, -0.02)],
                         [0.26, 0.26, 0.42], 0.90, 0.22, 0.085, glass)
    top_poly = [(-3.12, 2.23), (0.0, -0.78), (3.12, 2.23)]
    objs["top"] = slab("top", top_poly, [0.30, 0.52, 0.30], 1.22, 0.24, 0.09, glass)

    # warm light inside the envelope: an emitter in the air gap behind the top
    # flap, shaped like the flap so it is only ever seen through frosted glass
    centre = Vector((0.0, -0.12))
    outline = rounded(inset(top_poly, 0.16), [0.2, 0.4, 0.2], 16)
    me = bpy.data.meshes.new("glow")
    me.from_pydata([(p.x - centre.x, p.y - centre.y, 0) for p in outline], [], [list(range(len(outline)))])
    glow = bpy.data.objects.new("glow", me)
    sc.collection.objects.link(glow)
    glow.location = (centre.x, centre.y, 1.17)
    glow.data.materials.append(glow_material(2.1, 2.5))
    glow.visible_shadow = False
    objs["glow"] = glow
    # weaker spill behind the body so the lower flap and side panels warm up near the tip
    bpy.ops.mesh.primitive_circle_add(vertices=96, radius=1.0, fill_type="NGON", location=(0.0, -0.35, 0.32))
    spill = bpy.context.active_object
    spill.name = "spill"
    spill.data.materials.append(glow_material(1.6, 1.0))  # radius in object space
    spill.scale = (2.2, 1.6, 1.0)
    spill.visible_shadow = False
    spill.visible_diffuse = False  # keep it off the plate
    objs["spill"] = spill

    pl = bpy.data.lights.new("ember", "POINT")
    pl.shadow_soft_size = 0.6
    pl.energy = 150
    pl.color = lin("#FF9C70")[:3]
    po = bpy.data.objects.new("ember", pl)
    po.location = (0.0, -0.35, 1.16)
    # lights the inner faces only; never seen as a hot spot through the glass
    po.visible_camera = po.visible_transmission = po.visible_glossy = po.visible_diffuse = False
    sc.collection.objects.link(po)
    objs["ember"] = po

    return objs


def render(path):
    bpy.context.scene.render.filepath = path
    bpy.ops.render.render(write_still=True)
    print("WROTE", path)


def load_rgba(path):
    img = bpy.data.images.load(path, check_existing=False)
    w, h = img.size
    a = np.array(img.pixels[:], dtype=np.float32).reshape(h, w, 4)[::-1]  # top row first
    bpy.data.images.remove(img)
    return a


def save_rgba(arr, path):
    h, w = arr.shape[:2]
    img = bpy.data.images.new("out", w, h, alpha=True)
    img.pixels[:] = np.clip(arr[::-1], 0, 1).ravel()
    img.filepath_raw = path
    img.file_format = "PNG"
    img.save()
    bpy.data.images.remove(img)
    print("WROTE", path)


# ---------------------------------------------------------------- outputs

def do_icon():
    sc = reset()
    build(sc)
    render(os.path.join(HERE, "icon-%d.png" % RES))


def do_layers():
    # background layer: the velvet gradient on a full-bleed square, same light
    sc = reset()
    half = 5.12
    bpy.ops.mesh.primitive_plane_add(size=2 * half, location=(0, 0, 0))
    bpy.context.active_object.data.materials.append(plate_material(half))
    sc.render.film_transparent = False
    render(os.path.join(HERE, "layer-bg.png"))

    # envelope layer by difference matting: render the envelope over a black and
    # a grey unlit backdrop; alpha = 1 - (grey - black) / backdrop and the black
    # render is the premultiplied colour. Grey, not white, so the glow does not clip.
    tmp = os.path.join(bpy.app.tempdir or "/tmp", "skym")
    os.makedirs(tmp, exist_ok=True)
    shots = {}
    for key, col in (("k", "#000000"), ("w", "#BCBCBC")):
        sc = reset()
        o = build(sc)
        o["plate"].hide_render = True
        bpy.ops.mesh.primitive_plane_add(size=60, location=(0, 0, -0.8))
        bpy.context.active_object.data.materials.append(emission_material("matte", col))
        render(os.path.join(tmp, key + ".png"))
        shots[key] = os.path.join(tmp, key + ".png")
    # shadow: the plate as shadow catcher, envelope casts but stays unseen
    sc = reset()
    o = build(sc)
    o["plate"].is_shadow_catcher = True
    for k in ("body", "lower", "top", "glow", "spill"):
        o[k].visible_camera = False
    o["glow"].hide_render = True
    o["spill"].hide_render = True
    o["ember"].hide_render = True
    render(os.path.join(tmp, "s.png"))
    shots["s"] = os.path.join(tmp, "s.png")

    def to_lin(c):
        return np.where(c <= 0.04045, c / 12.92, ((c + 0.055) / 1.055) ** 2.4)

    def to_srgb(c):
        c = np.clip(c, 0, 1)
        return np.where(c <= 0.0031308, c * 12.92, 1.055 * c ** (1 / 2.4) - 0.055)

    k = to_lin(load_rgba(shots["k"])[..., :3])
    w = to_lin(load_rgba(shots["w"])[..., :3])
    backdrop = (w - k)[:8, :8].mean()  # a corner shows only the backdrop
    a_glass = np.clip(1.0 - (w - k).mean(axis=2) / backdrop, 0, 1)
    c_glass = k  # premultiplied colour of the glass and glow
    # the catcher reads darker than the shadow in the lit icon; 0.6 matches it by eye
    a_shadow = 0.6 * load_rgba(shots["s"])[..., 3]
    # shadow only where the glass does not cover it: glass over a black shadow layer
    a = a_glass + (1 - a_glass) * a_shadow
    straight = np.where(a[..., None] > 1e-4, c_glass / np.maximum(a[..., None], 1e-4), 0)
    out = np.dstack([to_srgb(straight), a])
    save_rgba(out, os.path.join(HERE, "layer-envelope.png"))


if MODE in ("icon", "all"):
    do_icon()
if MODE in ("layers", "all"):
    do_layers()
