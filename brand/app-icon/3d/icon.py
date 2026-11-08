"""Blender -b -P icon.py -- icon.png [--take2 | --take3 | --take4] (renders concept variants)."""
import bpy
import math
import random
import sys
from pathlib import Path
from mathutils import Vector, Matrix

out = Path(sys.argv[sys.argv.index('--') + 1]).resolve()
take4 = '--take4' in sys.argv
take3 = '--take3' in sys.argv or take4
take2 = '--take2' in sys.argv or take3
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
    if grain and (not take2 or (take3 and name == 'Brushed stainless')):
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
for i in range(2 if take2 else 9):
    angle = [-.16,.12,-.23,.07,.19,-.08,.13,-.04,.025][i]
    x = [.03,-.22,.16,-.1,.12,-.19,.05,-.03,0][i]
    y = .2 + (i%3-.8)*.11
    z = .29 + i*.06 if take2 else .07 + i*.038
    mat = white if take2 else red if i == 1 else green if i == 4 else paper if i%2 else white
    obj = box('Envelope' if i not in (1,4) else 'Task card', (x,y,z), (2.55,1.85,.03), mat, .035, angle)
    if i in ((1,) if take2 else (6,8)):
        def pt(a,b):
            return (x+a*math.cos(angle)-b*math.sin(angle),y+a*math.sin(angle)+b*math.cos(angle),z+.018)
        line('Envelope flap', [pt(-1.19,.82),pt(0,-.02),pt(1.19,.82)], seam,.018 if take2 else .006)
        line('Envelope lower folds', [pt(-1.19,-.82),pt(-.37,-.23)], seam,.004)
        line('Envelope lower folds', [pt(1.19,-.82),pt(.37,-.23)], seam,.004)
# A visible task corner, with empty boxes rather than letters or logos.
for j in range(0 if take2 else 3):
    y = -.15+j*.21
    line('Task checkbox', [(1.08,y,.278),(1.19,y,.278),(1.19,y+.10,.278),(1.08,y+.10,.278),(1.08,y,.278)], ink,.006)

# Shoulder-wide blade with a narrow rounded nose. The handle points toward +Y.
outline = [(-.18,.86),(-.30,.69),(-.94,.55),(-1.16,.38),(-1.16,.15),
           (-.76,-1.25),(-.65,-1.46),(-.43,-1.55),(.43,-1.55),(.65,-1.46),
           (.76,-1.25),(1.16,.15),(1.16,.38),(.94,.55),(.30,.69),(.18,.86)]
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
slot=box('Slot cutter',(0,.20,z0+.015),(2.02,.11,.4),steel,.044)
slot.modifiers[0].segments=16
bpy.context.view_layer.objects.active=slot
bpy.ops.object.modifier_apply(modifier=slot.modifiers[0].name)
bpy.context.view_layer.objects.active=blade
cut=blade.modifiers.new('Cutting aperture','BOOLEAN')
cut.operation='DIFFERENCE'
cut.object=slot
bpy.ops.object.modifier_apply(modifier=cut.name)
bpy.data.objects.remove(slot,do_unlink=True)
if take4:
    for vertex in blade.data.vertices:
        vertex.co.z += .035*(1-(vertex.co.x/1.16)**2)
    blade.data.update()
polished = material('Polished steel edge', (.65,.69,.72), .16, 1)
blade.data.materials.append(polished)
bevel(blade,.011,8)
blade.modifiers[0].material=1
blade.modifiers[0].harden_normals=True
blade.modifiers[1].keep_sharp=True
for face in blade.data.polygons:
    face.use_smooth = take4 or abs(face.normal.z) < .9
line('Polished cutting edge',[(x,.145,.477+(.035*(1-(x/1.16)**2) if take4 else 0)) for x in [-.96+i*1.92/32 for i in range(33)]],polished,.004)


def between(name,a,b,radius,mat):
    direction=Vector(b)-Vector(a)
    bpy.ops.mesh.primitive_cylinder_add(vertices=64,radius=radius,depth=direction.length,location=(Vector(a)+Vector(b))/2)
    o=bpy.context.object
    o.name=name
    o.rotation_euler=direction.to_track_quat('Z','Y').to_euler()
    o.data.materials.append(mat)
    bevel(o,.075,8)
    for p in o.data.polygons: p.use_smooth=True
    return o

# A flattened root rolls smoothly into the round rising neck.
neck_profile=[(.66,.478,.18,.016),(.70,.485,.17,.025),(.76,.50,.145,.045),
              (.83,.53,.12,.075),(.92,.575,.108,.095),(1.05,.64,.108,.105),
              (1.18,.71,.11,.11),(1.32,.77,.115,.115)]
verts=[]
for y,z,rx,rz in neck_profile:
    for j in range(64):
        a=2*math.pi*j/64
        verts.append((rx*math.cos(a),y,z+rz*math.sin(a)))
faces=[(i*64+j,i*64+(j+1)%64,(i+1)*64+(j+1)%64,(i+1)*64+j)
       for i in range(len(neck_profile)-1) for j in range(64)]
neck=mesh('Flowing pressed neck',verts,faces,steel)
sub=neck.modifiers.new('Smooth neck transition','SUBSURF'); sub.levels=2
for face in neck.data.polygons: face.use_smooth=True
between('Steel ferrule',(0,1.18,.70),(0,1.48,.86),.19,steel)
between('Walnut grip',(0,1.43,.83),(0,2.50,1.34) if take2 else (0,2.92,1.53),.24 if take2 else .205,wood)
if not take2:
    between('End cap',(0,2.90,1.52),(0,2.97,1.55),.19,steel)

# The letter emerges through the slot and coils 1.25 turns above the blade.
profile=[(.20,.43),(.17,.61),(.13,.81)]
def curl(t, offset=0):
    radius=.43-.18*t/(2.5*math.pi)+offset
    return (-.30+radius*math.cos(t), .98+radius*math.sin(t))
profile += [curl(2.5*math.pi*i/160) for i in range(161)]
v=[(x,y,z) for y,z in profile for x in (-.58,.58)]
strip=mesh('Single curled letter',v,[(2*i,2*i+1,2*i+3,2*i+2) for i in range(len(profile)-1)],white)
sub=strip.modifiers.new('Relaxed paper curl','SUBSURF'); sub.levels=2
solid=strip.modifiers.new('Paper thickness','SOLIDIFY'); solid.thickness=.008
for face in strip.data.polygons: face.use_smooth=True
# Rules sit on the outer face of the spiral, with no letters or glyphs.
for i,t in enumerate([.7, 1.05, 1.4, 1.75] if take4 else [.8, 1.4, 2.0] if take2 else [.65, .92, 1.19, 1.46, 1.73, 2.0, 2.27, 2.54, 2.81, 3.08, 3.35]):
    y,z=curl(t,.012 if take4 else .009)
    line('Curled letter rule',[(-.48,y,z),([.35,.20,.40,.05][i] if take4 else .40 if i%4!=3 else .12,y,z)],ink,.003 if take4 else .030 if take2 else .007)
if take4:
    # Reveal the rolled edge while keeping the slicer itself symmetric.
    pivot = Vector((0,-.30,.98))
    turn = Matrix.Rotation(math.radians(18),4,'Y')
    transform = Matrix.Translation(pivot) @ turn @ Matrix.Translation(-pivot)
    for obj in scene.objects:
        if obj.name.startswith(('Single curled letter','Curled letter rule')):
            obj.matrix_world = transform @ obj.matrix_world
subject=list(scene.objects)

# Orthographic camera and broad softboxes: no external textures or environment maps.
bpy.ops.object.camera_add(location=(0,-5,18) if take4 else (5,-8,14))
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

# Frame evaluated subject geometry at 660 px, independently of the 824 px plate.
bpy.context.view_layer.update()
deps=bpy.context.evaluated_depsgraph_get()
view=cam.matrix_world.inverted()
points=[]
for obj in subject:
    evaluated=obj.evaluated_get(deps)
    geometry=evaluated.to_mesh()
    if geometry:
        points.extend(view @ evaluated.matrix_world @ v.co for v in geometry.vertices)
        evaluated.to_mesh_clear()
lo=Vector((min(p.x for p in points),min(p.y for p in points),0))
hi=Vector((max(p.x for p in points),max(p.y for p in points),0))
scale=660/(max(hi.x-lo.x,hi.y-lo.y)*1024/cam.data.ortho_scale)
center=(lo+hi)/2
root=bpy.data.objects.new('Optically centered subject',None)
scene.collection.objects.link(root)
for obj in subject: obj.parent=root
root.scale=(scale,)*3
# Camera-space origin is the framing target, not the world origin.
rotation=cam.rotation_euler.to_matrix()
target=Vector((0,.55,.35))
root.location=target*(1-scale)+rotation @ Vector((-scale*center.x,-scale*center.y,0))
assert 1 < scale < 1.8
print('Subject maximum extent: 660 px; scale',round(scale,4),flush=True)

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
if take2:
    nodes, links = plate_mat.node_tree.nodes, plate_mat.node_tree.links
    coord = nodes.new('ShaderNodeTexCoord')
    separate = nodes.new('ShaderNodeSeparateXYZ')
    gradient = nodes.new('ShaderNodeValToRGB')
    links.new(coord.outputs['Generated'], separate.inputs[0])
    links.new(separate.outputs['Y'], gradient.inputs[0])
    links.new(gradient.outputs[0], nodes.get('Principled BSDF').inputs['Base Color'])
    nodes.get('Principled BSDF').inputs['Roughness'].default_value = .32
    ink.node_tree.nodes.get('Principled BSDF').inputs['Base Color'].default_value = (.12,.17,.22,1)
    if take3:
        # Suppress pale softbox reflections on the red tile, retaining contact shadows.
        bs = nodes.get('Principled BSDF')
        bs.inputs['Roughness'].default_value = 1
        bs.inputs['Specular IOR Level'].default_value = 0
        gradient.color_ramp.elements[0].color = (.24,.009,.005,1)
        gradient.color_ramp.elements[1].color = (.37,.020,.010,1)
        wood.node_tree.nodes.get('Principled BSDF').inputs['Base Color'].default_value = (.045,.022,.016,1)
        if take4:
            links.new(gradient.outputs[0], bs.inputs['Emission Color'])
            bs.inputs['Emission Strength'].default_value = .65
            ink.node_tree.nodes.get('Principled BSDF').inputs['Base Color'].default_value = (.065,.065,.065,1)
        frames = [('1',2.18,(0,-.35,.65)),('2',2.18,(0,-.35,.65))] if take4 else [
            ('1', 2.65, (0,-.12,.68)),
            ('2', 4.0, (0,-.02,.93)),
            ('3', 2.65, (0,-.12,.68))]
        for variant, zoom, anchor in frames:
            for obj in subject:
                obj.hide_render = (variant != '3' and obj.name.startswith(('Envelope','Task'))) or (not take4 and variant == '2' and obj.name.startswith(('Flowing','Steel ferrule','Walnut grip')))
            root.scale = (zoom,)*3
            root.rotation_euler.z = math.radians(-10) if take4 and variant == '2' else 0
            root.location = target - zoom*(root.rotation_euler.to_matrix() @ Vector(anchor))
            plate.location = target + rotation @ Vector((0,0,-3.5))
            scene.render.filepath = str(out.with_name(('take4-' if take4 else 'take3-')+variant+'.png'))
            bpy.ops.render.render(write_still=True)
            # Clip the zoomed subject to the same rounded 824 px tile silhouette.
            rendered = bpy.data.images.load(scene.render.filepath, check_existing=False)
            pixels = list(rendered.pixels)
            for y in range(1024):
                for x in range(1024):
                    dx, dy = max(abs(x+.5-512)-282,0), max(abs(y+.5-512)-282,0)
                    coverage = max(0,min(1,130.5-math.hypot(dx,dy)))
                    pixels[(y*1024+x)*4+3] *= coverage
                    if coverage == 0:
                        pixels[(y*1024+x)*4:(y*1024+x)*4+3] = (0,0,0)
            rendered.pixels = pixels
            rendered.filepath_raw = scene.render.filepath
            rendered.save()
            bpy.data.images.remove(rendered)
        assert all(out.with_name(('take4-' if take4 else 'take3-')+v+'.png').exists() for v in ('12' if take4 else '123'))
    else:
        for variant, color, grip in [
            ('A', (.43,.035,.022), (.045,.022,.016)),
            ('B', (.012,.065,.34), (.16,.065,.025)),
            ('C', (.038,.032,.029), (.43,.035,.022))]:
            gradient.color_ramp.elements[0].color = (*(v*.65 for v in color),1)
            gradient.color_ramp.elements[1].color = (*(v*1.35 for v in color),1)
            wood.node_tree.nodes.get('Principled BSDF').inputs['Base Color'].default_value = (*grip,1)
            scene.render.filepath = str(out.with_name('take2-'+variant+'.png'))
            bpy.ops.render.render(write_still=True)
        assert all(out.with_name('take2-'+v+'.png').exists() for v in 'ABC')

else:
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
