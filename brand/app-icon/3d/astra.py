"""Procedural studio icon. Blender -b -P astra.py -- astra.png"""
import bpy
import math
import random
import sys
from pathlib import Path
from mathutils import Vector

out = Path(sys.argv[sys.argv.index('--') + 1]).resolve()
assert out.suffix == '.png'
random.seed(17)
bpy.ops.wm.read_factory_settings(use_empty=True)
scene = bpy.context.scene
scene.render.engine = 'CYCLES'
scene.cycles.samples = 96
scene.cycles.use_denoising = True
scene.render.resolution_x = scene.render.resolution_y = 1024
scene.render.resolution_percentage = 100
scene.render.image_settings.file_format = 'PNG'
scene.render.image_settings.color_mode = 'RGBA'
scene.render.film_transparent = True
scene.render.use_stamp = False
scene.view_settings.view_transform = 'AgX'
scene.world = bpy.data.worlds.new('Studio world')
scene.world.color = (.18, .18, .18)


def material(name, color, rough=.4, metal=0, grain=None):
    m = bpy.data.materials.new(name)
    m.diffuse_color = (*color, 1)
    m.use_nodes = True
    n = m.node_tree.nodes
    l = m.node_tree.links
    bs = n.get('Principled BSDF')
    bs.inputs['Base Color'].default_value = (*color, 1)
    bs.inputs['Roughness'].default_value = rough
    bs.inputs['Metallic'].default_value = metal
    if metal:
        bs.inputs['Anisotropic'].default_value = .72
    if grain:
        coord = n.new('ShaderNodeTexCoord')
        mapping = n.new('ShaderNodeVectorMath')
        mapping.operation = 'MULTIPLY'
        mapping.inputs[1].default_value = grain
        l.new(coord.outputs['Generated'], mapping.inputs[0])
        noise = n.new('ShaderNodeTexNoise')
        noise.inputs['Scale'].default_value = 8
        noise.inputs['Detail'].default_value = 3
        l.new(mapping.outputs[0], noise.inputs['Vector'])
        bump = n.new('ShaderNodeBump')
        bump.inputs['Strength'].default_value = .16
        bump.inputs['Distance'].default_value = .012 if metal else .018
        l.new(noise.outputs['Fac'], bump.inputs['Height'])
        l.new(bump.outputs[0], bs.inputs['Normal'])
        if name == 'Walnut':
            ramp = n.new('ShaderNodeValToRGB')
            ramp.color_ramp.elements[0].color = (.025, .009, .003, 1)
            ramp.color_ramp.elements[1].color = (.24, .095, .027, 1)
            l.new(noise.outputs['Fac'], ramp.inputs[0])
            l.new(ramp.outputs[0], bs.inputs['Base Color'])
    return m

steel = material('Brushed stainless', (.52, .57, .60), .27, 1, (150, 1, 2))
wood = material('Walnut', (.14, .055, .018), .3, grain=(12, .3, 12))
paper = material('Cotton paper', (.83, .80, .71), .78, grain=(80, 80, 15))
white = material('Fresh paper', (.96, .94, .86), .69, grain=(100, 100, 20))
ink = material('Faint printed rules', (.43, .47, .44), .8)
seam = material('Fold shadow', (.56, .54, .49), .85)
red = material('Terracotta card', (.35, .075, .045), .7, grain=(50,50,10))
green = material('Sage card', (.17, .26, .19), .7, grain=(50,50,10))


def bevel(obj, width, segments=4):
    mod = obj.modifiers.new('Soft manufactured edges', 'BEVEL')
    mod.width, mod.segments = width, segments
    obj.modifiers.new('Weighted normals', 'WEIGHTED_NORMAL')
    return obj


def box(name, loc, size, mat, radius=.02, rot=0):
    bpy.ops.mesh.primitive_cube_add(size=1, location=loc)
    o = bpy.context.object
    o.name = name
    o.dimensions = size
    bpy.ops.object.transform_apply(location=False, rotation=False, scale=True)
    o.data.materials.append(mat)
    bevel(o, radius)
    o.rotation_euler.z = rot
    return o


def mesh(name, vertices, faces, mat):
    data = bpy.data.meshes.new(name)
    data.from_pydata(vertices, [], faces)
    data.update()
    o = bpy.data.objects.new(name, data)
    scene.collection.objects.link(o)
    o.data.materials.append(mat)
    return o


def line(name, points, mat, width=.008):
    c = bpy.data.curves.new(name, 'CURVE')
    c.dimensions = '3D'
    c.bevel_depth = width
    c.bevel_resolution = 3
    s = c.splines.new('POLY')
    s.points.add(len(points)-1)
    for p, v in zip(s.points, points):
        p.co = (*v, 1)
    o = bpy.data.objects.new(name,c)
    scene.collection.objects.link(o)
    o.data.materials.append(mat)
    return o


# A loose stack: real thickness, visible folded envelope backs, and two task cards.
for i in range(9):
    angle = [-.16,.12,-.23,.07,.19,-.08,.13,-.04,.025][i]
    x = [.03,-.22,.16,-.1,.12,-.19,.05,-.03,0][i]
    y = .2 + (i%3-.8)*.11
    z = .07 + i*.038
    mat = red if i == 1 else green if i == 4 else paper if i%2 else white
    obj = box('Envelope' if i not in (1,4) else 'Task card', (x,y,z), (2.55,1.85,.03), mat, .035, angle)
    if i in (6,8):
        def pt(a,b):
            return (x+a*math.cos(angle)-b*math.sin(angle),y+a*math.sin(angle)+b*math.cos(angle),z+.018)
        line('Envelope flap', [pt(-1.19,.82),pt(0,-.02),pt(1.19,.82)], seam,.006)
        line('Envelope lower folds', [pt(-1.19,-.82),pt(-.37,-.23)], seam,.004)
        line('Envelope lower folds', [pt(1.19,-.82),pt(.37,-.23)], seam,.004)
# A visible task corner, with empty boxes rather than letters or logos.
for j in range(3):
    y = -.15+j*.21
    line('Task checkbox', [(1.08,y,.278),(1.19,y,.278),(1.19,y+.10,.278),(1.08,y+.10,.278),(1.08,y,.278)], ink,.006)

# Shoulder-wide blade with a narrow rounded nose. The handle points toward +Y.
outline = [(-.18,.86),(-.30,.69),(-.83,.55),(-1.02,.38),(-1.03,.15),
           (-.76,-1.25),(-.65,-1.46),(-.43,-1.55),(.43,-1.55),(.65,-1.46),
           (.76,-1.25),(1.03,.15),(1.02,.38),(.83,.55),(.30,.69),(.18,.86)]
# Corner cutting makes a continuous pressed outline without polygonal shoulders.
for _ in range(4):
    outline = [point for a,b in zip(outline, outline[1:]+outline[:1])
               for point in ((.75*a[0]+.25*b[0], .75*a[1]+.25*b[1]),
                             (.25*a[0]+.75*b[0], .25*a[1]+.75*b[1]))]
# At 40 mm per scene unit this is 1.2 mm steel; keep the original top surface.
z0=.445
thickness=.030
verts=[(x,y,z0+z) for z in (0,thickness) for x,y in outline]
n=len(outline)
faces=[tuple(reversed(range(n))),tuple(range(n,2*n))]+[(i,(i+1)%n,(i+1)%n+n,i+n) for i in range(n)]
blade=mesh('Tapered steel blade',verts,faces,steel)
# Boolean makes an actual aperture, not a dark line painted on solid steel.
slot=box('Slot cutter',(0,.20,z0+.015),(1.76,.09,.4),steel,.044)
slot.modifiers[0].segments=16
bpy.context.view_layer.objects.active=slot
bpy.ops.object.modifier_apply(modifier=slot.modifiers[0].name)
bpy.context.view_layer.objects.active=blade
cut=blade.modifiers.new('Cutting aperture','BOOLEAN')
cut.operation='DIFFERENCE'
cut.object=slot
bpy.ops.object.modifier_apply(modifier=cut.name)
bpy.data.objects.remove(slot,do_unlink=True)
polished = material('Polished steel edge', (.65,.69,.72), .16, 1)
blade.data.materials.append(polished)
bevel(blade,.006,6)
blade.modifiers[0].material=1
blade.modifiers[0].harden_normals=True
blade.modifiers[1].keep_sharp=True
for face in blade.data.polygons:
    face.use_smooth = abs(face.normal.z) < .9
line('Polished cutting edge',[(-.83,.155,.477),(.83,.155,.477)],polished,.004)


def between(name,a,b,radius,mat):
    direction=Vector(b)-Vector(a)
    bpy.ops.mesh.primitive_cylinder_add(vertices=64,radius=radius,depth=direction.length,location=(Vector(a)+Vector(b))/2)
    o=bpy.context.object
    o.name=name
    o.rotation_euler=direction.to_track_quat('Z','Y').to_euler()
    o.data.materials.append(mat)
    bevel(o,.045,5)
    for p in o.data.polygons: p.use_smooth=True
    return o

between('Raised steel neck',(0,.70,.47),(0,1.32,.77),.115,steel)
between('Steel ferrule',(0,1.18,.70),(0,1.48,.86),.19,steel)
between('Walnut grip',(0,1.43,.83),(0,2.92,1.53),.205,wood)
between('End cap',(0,2.90,1.52),(0,2.97,1.55),.19,steel)

# Sheet crosses the aperture, then relaxes onto the blade toward its tip.
profile=[(.27,.36),(.20,.46),(.10,.61),(-.06,.70),(-.27,.68),(-.54,.56),(-.83,.52),(-1.12,.56)]
v=[]
for y,z in profile:
    for x in (-.69,.69): v.append((x,y,z))
strip=mesh('Single shaved letter',v,[(2*i,2*i+1,2*i+3,2*i+2) for i in range(len(profile)-1)],white)
sub=strip.modifiers.new('Relaxed paper curl','SUBSURF'); sub.levels=3
solid=strip.modifiers.new('Paper thickness','SOLIDIFY'); solid.thickness=.008
for p in strip.data.polygons: p.use_smooth=True
# Printed lines follow the quiet, nearly flat part; intentionally no glyphs.
for i,(y,z) in enumerate([(-.55,.563),(-.67,.538),(-.79,.529),(-.91,.54)]):
    line('Subtle letter rule',[(-.48,y,z+.009),(.40 if i<3 else .10,y,z+.009)],ink,.008)

# Orthographic camera and broad softboxes: no external textures or environment maps.
bpy.ops.object.camera_add(location=(5,-8,14))
cam=bpy.context.object
cam.rotation_euler=(Vector((0,.55,.35))-cam.location).to_track_quat('-Z','Y').to_euler()
cam.data.type='ORTHO'; cam.data.ortho_scale=7.5
scene.camera=cam
for name,loc,power,sz,color in [
    ('Large warm key',(-4,-3,8),850,5,(1,.91,.79)),
    ('Cool strip reflection',(4,2,6),1000,4,(.79,.88,1)),
    ('Front fill',(1,-5,4),250,3,(1,1,1))]:
    bpy.ops.object.light_add(type='AREA',location=loc)
    o=bpy.context.object; o.name=name; o.data.energy=power; o.data.shape='RECTANGLE'
    o.data.size=sz; o.data.size_y=sz*.65; o.data.color=color
    o.rotation_euler=(Vector((0,0,.3))-o.location).to_track_quat('-Z','Y').to_euler()

# A camera-facing rounded square keeps the macOS silhouette inside the 824 px grid.
plate_mat=material('Icon plate',(.79,.76,.68),.62)
rim=[]
for cx,cy,start in [(2.065,2.065,0),(-2.065,2.065,90),(-2.065,-2.065,180),(2.065,-2.065,270)]:
    for step in range(17):
        angle=math.radians(start+step*90/16)
        rim.append((cx+.95*math.cos(angle),cy+.95*math.sin(angle),0))
plate=mesh('Rounded icon plate',rim,[tuple(range(len(rim)))],plate_mat)
solid=plate.modifiers.new('Plate depth','SOLIDIFY'); solid.thickness=.12
bevel(plate,.045,4)
plate.rotation_euler=cam.rotation_euler
plate.location=Vector((0,.55,.35)) + cam.rotation_euler.to_matrix() @ Vector((0,0,-1.55))
assert len(outline)==256 and .025 <= thickness <= .0375
assert max(abs(x) for x,y in outline if y>0) > max(abs(x) for x,y in outline if y<-1.3)
assert len(profile)>3 and scene.render.resolution_x==1024
for variant,color in [('light',(.79,.76,.68)),('dark',(.027,.035,.032))]:
    plate_mat.node_tree.nodes.get('Principled BSDF').inputs['Base Color'].default_value=(*color,1)
    scene.render.filepath=str(out.with_name(out.stem+'-'+variant+'.png'))
    bpy.ops.render.render(write_still=True)
# The main asset is an isolated transparent object; the thumbnail previews the light icon.
plate.hide_render=True
scene.render.filepath=str(out)
bpy.ops.render.render(write_still=True)
image=bpy.data.images.load(str(out.with_name(out.stem+'-light.png')))
assert len(image.pixels) == 1024 * 1024 * 4
image.pixels[0]  # Load pixels before changing the source path.
image.scale(32,32)
image.filepath_raw=str(out.with_name(out.stem+'-32.png')); image.save()
assert all(out.with_name(out.stem+s+'.png').exists() for s in ('','-light','-dark','-32'))
