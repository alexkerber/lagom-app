"""Blender -b -P render-3d.py -- output-directory"""
import math
import sys
import subprocess
from pathlib import Path
import bpy
from mathutils import Vector

out = Path(sys.argv[sys.argv.index('--')+1]).resolve()
assert out.is_dir()

def rgb(value):
    c = [int(value[i:i+2],16)/255 for i in (0,2,4)]
    return tuple(v/12.92 if v <= .04045 else ((v+.055)/1.055)**2.4 for v in c)

def mat(name, color, rough=.5):
    m=bpy.data.materials.new(name); m.use_nodes=True
    bs=m.node_tree.nodes.get('Principled BSDF')
    bs.inputs['Base Color'].default_value=(*rgb(color),1)
    bs.inputs['Roughness'].default_value=rough
    bs.inputs['Specular IOR Level'].default_value=.25
    return m

def point(x,y): return ((x-512)/100,(512-y)/100)

def rounded(x,y,w,h,r):
    points=[]
    for cx,cy,start in [(x+w-r,y+h-r,0),(x+r,y+h-r,90),(x+r,y+r,180),(x+w-r,y+r,270)]:
        for i in range(17):
            a=math.radians(start+i*90/16)
            points.append((cx+r*math.cos(a),cy+r*math.sin(a)))
    return points

def sheet(name, outline, surface, material, thickness, edge=.016, subdivide=False):
    data=bpy.data.meshes.new(name)
    data.from_pydata([(x,y,0) for x,y in outline],[],[tuple(range(len(outline)))])
    data.update()
    obj=bpy.data.objects.new(name,data); bpy.context.collection.objects.link(obj)
    obj.data.materials.append(material)
    bpy.ops.object.select_all(action='DESELECT')
    bpy.context.view_layer.objects.active=obj; obj.select_set(True)
    if subdivide:
        mod=obj.modifiers.new('Surface triangles','TRIANGULATE')
        bpy.ops.object.modifier_apply(modifier=mod.name)
        bpy.ops.object.mode_set(mode='EDIT'); bpy.ops.mesh.select_all(action='SELECT')
        bpy.ops.mesh.subdivide(number_cuts=5)
        bpy.ops.object.mode_set(mode='OBJECT')
    for v in obj.data.vertices: v.co.z=surface(v.co.x,v.co.y)
    obj.data.update()
    for p in obj.data.polygons: p.use_smooth=True
    solid=obj.modifiers.new('Paper thickness','SOLIDIFY'); solid.thickness=thickness; solid.offset=-1
    bevel=obj.modifiers.new('Soft edge','BEVEL'); bevel.width=edge; bevel.segments=3; bevel.limit_method='ANGLE'
    obj.select_set(False)
    return obj

def ink_stroke(name, points, radius, surface, material):
    # A flattened ribbon keeps the mark printed/embossed rather than tubular.
    c=bpy.data.curves.new(name,'CURVE'); c.dimensions='3D'
    c.bevel_depth=radius; c.bevel_resolution=5; c.resolution_u=12; c.use_fill_caps=True
    s=c.splines.new('POLY'); s.points.add(len(points)-1)
    for p,(x,y) in zip(s.points,points): p.co=(x,y,0,1)
    obj=bpy.data.objects.new(name,c); bpy.context.collection.objects.link(obj)
    bpy.ops.object.select_all(action='DESELECT')
    bpy.context.view_layer.objects.active=obj; obj.select_set(True)
    bpy.ops.object.convert(target='MESH')
    for v in obj.data.vertices: v.co.z=surface(v.co.x,v.co.y)+.012+v.co.z*.045
    obj.data.materials.append(material)
    for f in obj.data.polygons: f.use_smooth=True
    obj.select_set(False)
    for x,y in points:
        ring=[(x+radius*math.cos(i*math.tau/64),y+radius*math.sin(i*math.tau/64)) for i in range(64)]
        sheet(name+' rounded join',ring,lambda a,b:surface(a,b)+.017,material,.012,.006,True)

def setup(top,bottom):
    bpy.ops.wm.read_factory_settings(use_empty=True)
    scene=bpy.context.scene
    scene.render.engine='CYCLES'; scene.cycles.samples=96; scene.cycles.use_denoising=True
    scene.render.resolution_x=scene.render.resolution_y=1024
    scene.render.resolution_percentage=100; scene.render.film_transparent=True
    scene.render.image_settings.file_format='PNG'; scene.render.image_settings.color_mode='RGBA'
    scene.view_settings.view_transform='AgX'
    scene.view_settings.look='AgX - Medium High Contrast'
    scene.view_settings.exposure=.3
    scene.world=bpy.data.worlds.new('Studio'); scene.world.color=(.2,.2,.2)
    plate=mat('Green icon tile',bottom,.6)
    nodes=plate.node_tree.nodes; links=plate.node_tree.links
    coord=nodes.new('ShaderNodeTexCoord'); split=nodes.new('ShaderNodeSeparateXYZ'); ramp=nodes.new('ShaderNodeValToRGB')
    links.new(coord.outputs['Generated'],split.inputs[0]); links.new(split.outputs['Y'],ramp.inputs[0])
    ramp.color_ramp.elements[0].color=(*rgb(bottom),1); ramp.color_ramp.elements[1].color=(*rgb(top),1)
    bs=nodes.get('Principled BSDF'); bs.inputs['Specular IOR Level'].default_value=.08
    links.new(ramp.outputs[0],bs.inputs['Base Color']); links.new(ramp.outputs[0],bs.inputs['Emission Color'])
    bs.inputs['Emission Strength'].default_value=.35
    sheet('Rounded tile',rounded(-4.12,-4.12,8.24,8.24,1.85),lambda x,y:0,plate,.08,.012)
    bpy.ops.object.camera_add(location=(0,0,20))
    cam=bpy.context.object; cam.rotation_euler=(Vector((0,0,0))-cam.location).to_track_quat('-Z','Y').to_euler()
    cam.data.type='ORTHO'; cam.data.ortho_scale=10.24; scene.camera=cam
    for loc,power,size in [((-4,5,9),850,5),((5,1,7),350,5),((0,-5,6),100,4)]:
        bpy.ops.object.light_add(type='AREA',location=loc)
        lamp=bpy.context.object; lamp.data.energy=power; lamp.data.shape='DISK'; lamp.data.size=size
        lamp.rotation_euler=(-lamp.location).to_track_quat('-Z','Y').to_euler()
    return scene

scene=setup('1F6B5A','0E3B33')
paper=mat('Thick ivory paper','F8F5EF')
# Circular scallops are actual cuts in the extruded paper boundary.
assert 2.8-2.4 > .26*math.sqrt(2)  # Adjacent corner scallops must not overlap.
edge=[(-2.8,-2.8)]
for i in range(9):
    cx=-2.40+4.80*i/8
    for j in range(21):
        a=math.pi-j*math.pi/20
        edge.append((cx+.26*math.cos(a),-2.8+.26*math.sin(a)))
edge.append((2.8,-2.8))
outline=[]
for k in range(4):
    a=k*math.pi/2
    outline.extend((x*math.cos(a)-y*math.sin(a),x*math.sin(a)+y*math.cos(a)) for x,y in edge[:-1])
assert len(outline)==760

def stamp_surface(x,y): return .20+.33*max(0,(x+y-2.2)/3.4)**2
sheet('Perforated stamp',outline,stamp_surface,paper,.085,.012,True)
# Print directly on the bent surface so independent meshes cannot intersect.
print_path=out.parents[2]/'target'/'icon-concepts'/'stamp-print.png'
print_path.parent.mkdir(parents=True,exist_ok=True)
print_svg='''<svg xmlns="http://www.w3.org/2000/svg" viewBox="232 232 560 560">
<rect x="232" y="232" width="560" height="560" fill="#F8F5EF"/>
<rect x="302" y="302" width="420" height="420" rx="28" fill="#E8735A"/>
<path d="M392 520L478 604L640 422" fill="none" stroke="#FFFFFF" stroke-width="58" stroke-linecap="round" stroke-linejoin="round"/>
</svg>'''
subprocess.run(['rsvg-convert','-w','1024','-h','1024','-o',str(print_path)],input=print_svg,text=True,check=True)
nodes=paper.node_tree.nodes; links=paper.node_tree.links
texture=nodes.new('ShaderNodeTexImage'); texture.image=bpy.data.images.load(str(print_path))
coord=nodes.new('ShaderNodeTexCoord')
links.new(coord.outputs['Generated'],texture.inputs['Vector'])
links.new(texture.outputs['Color'],nodes.get('Principled BSDF').inputs['Base Color'])
scene.render.filepath=str(out/'3d-stamp.png'); bpy.ops.render.render(write_still=True)

scene=setup('2E9A6E','145C41')
paper=mat('Envelope paper','F8F5EF'); fold=mat('Ivory folded flap','FFFDF7'); green=mat('Green folded check','145C41',.45)
sheet('Envelope body',rounded(-2.9,-2.1,5.8,4.1,.56),lambda x,y:.21,paper,.12,.035)
# The asymmetric flap edge follows the same three points as the green check.
flap=[point(x,y) for x,y in [(262,352),(340,500),(462,618),(716,372),(762,352)]]
def flap_surface(x,y): return .28+.12*max(0,(1.6-y)/2.66)
sheet('Raised envelope flap',flap,flap_surface,fold,.045,.018,True)
ink_stroke('Check-shaped flap crease',[point(340,500),point(462,618),point(716,372)],.32,flap_surface,green)
scene.render.filepath=str(out/'3d-foldcheck.png'); bpy.ops.render.render(write_still=True)
assert all((out/name).exists() for name in ('3d-stamp.png','3d-foldcheck.png'))
