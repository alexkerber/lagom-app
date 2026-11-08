"""Blender -b -P render-hero.py -- brand/app-icon/concepts [A|B|C]."""
import math
import sys
from pathlib import Path
import bpy
from mathutils import Vector

args=sys.argv[sys.argv.index('--')+1:]
out=Path(args[0]).resolve()
variants=args[1:] or ['A','B','C']
assert out.is_dir() and all(v in 'ABC' and len(v)==1 for v in variants)

def rgb(h):
    values=[int(h[i:i+2],16)/255 for i in (0,2,4)]
    return tuple(v/12.92 if v <= .04045 else ((v+.055)/1.055)**2.4 for v in values)

def surface(name,color,rough=.2,transmission=0,ior=1.45,emission=0,absorption=0):
    m=bpy.data.materials.new(name); m.use_nodes=True
    n=m.node_tree.nodes; l=m.node_tree.links; bs=n.get('Principled BSDF')
    bs.inputs['Base Color'].default_value=(*rgb(color),1)
    bs.inputs['Roughness'].default_value=rough
    bs.inputs['Transmission Weight'].default_value=transmission
    bs.inputs['IOR'].default_value=ior
    bs.inputs['Coat Weight'].default_value=.22 if transmission else .1
    bs.inputs['Coat Roughness'].default_value=.055
    bs.inputs['Emission Color'].default_value=(*rgb(color),1)
    bs.inputs['Emission Strength'].default_value=emission
    if absorption:
        volume=n.new('ShaderNodeVolumeAbsorption')
        volume.inputs['Color'].default_value=(*rgb(color),1)
        volume.inputs['Density'].default_value=absorption
        l.new(volume.outputs[0],n.get('Material Output').inputs['Volume'])
    return m

def rounded_box(name,center,size,radius,material,layer):
    bpy.ops.mesh.primitive_cube_add(size=1,location=center)
    obj=bpy.context.object; obj.name=name; obj.dimensions=size
    bpy.ops.object.transform_apply(location=False,rotation=False,scale=True)
    bevel=obj.modifiers.new('Polished rounded edge','BEVEL'); bevel.width=radius; bevel.segments=16
    bpy.ops.object.modifier_apply(modifier=bevel.name)
    for p in obj.data.polygons: p.use_smooth=True
    normal=obj.modifiers.new('Face normals','WEIGHTED_NORMAL'); normal.keep_sharp=True
    if material: obj.data.materials.append(material)
    obj['icon_layer']=layer
    return obj

def difference(obj,cutter):
    bpy.context.view_layer.objects.active=obj
    for previous in list(obj.modifiers):
        if previous.type=='BEVEL': bpy.ops.object.modifier_apply(modifier=previous.name)
    mod=obj.modifiers.new('Physical cavity','BOOLEAN'); mod.operation='DIFFERENCE'; mod.solver='EXACT'; mod.object=cutter
    bpy.ops.object.modifier_move_up(modifier=mod.name)
    bpy.ops.object.modifier_apply(modifier=mod.name)
    bpy.data.objects.remove(cutter,do_unlink=True)

def sphere(name,center,scale,material,layer):
    bpy.ops.mesh.primitive_uv_sphere_add(segments=128,ring_count=64,radius=1,location=center)
    obj=bpy.context.object; obj.name=name; obj.scale=scale
    bpy.ops.object.transform_apply(location=False,rotation=False,scale=True)
    for p in obj.data.polygons: p.use_smooth=True
    if material: obj.data.materials.append(material)
    obj['icon_layer']=layer
    return obj

def prism(name,points,z,depth,material,layer,bevel=.06):
    n=len(points); verts=[(x,y,z+d) for d in (0,depth) for x,y in points]
    faces=[tuple(reversed(range(n))),tuple(range(n,2*n))]+[(i,(i+1)%n,(i+1)%n+n,i+n) for i in range(n)]
    mesh=bpy.data.meshes.new(name); mesh.from_pydata(verts,[],faces); mesh.update()
    obj=bpy.data.objects.new(name,mesh); bpy.context.collection.objects.link(obj)
    obj.data.materials.append(material); obj['icon_layer']=layer
    mod=obj.modifiers.new('Soft glass seam','BEVEL'); mod.width=bevel; mod.segments=8
    obj.modifiers.new('Face normals','WEIGHTED_NORMAL')
    return obj

def setup(top,bottom):
    bpy.ops.wm.read_factory_settings(use_empty=True)
    scene=bpy.context.scene; scene.render.engine='CYCLES'
    prefs=bpy.context.preferences.addons['cycles'].preferences
    prefs.compute_device_type='METAL'; prefs.get_devices()
    for device in prefs.devices: device.use=device.type=='METAL'
    scene.cycles.device='GPU' if any(d.use for d in prefs.devices) else 'CPU'
    scene.cycles.samples=512; scene.cycles.adaptive_threshold=.003
    scene.cycles.use_denoising=True; scene.cycles.denoising_prefilter='ACCURATE'
    scene.cycles.max_bounces=16; scene.cycles.transmission_bounces=12
    scene.cycles.glossy_bounces=8; scene.cycles.transparent_max_bounces=16
    scene.cycles.caustics_reflective=True; scene.cycles.caustics_refractive=True
    scene.cycles.seed=31
    scene.render.resolution_x=scene.render.resolution_y=1024; scene.render.resolution_percentage=100
    scene.render.image_settings.file_format='PNG'; scene.render.image_settings.color_mode='RGBA'
    scene.render.film_transparent=True
    scene.view_settings.view_transform='AgX'; scene.view_settings.look='AgX - Medium High Contrast'
    scene.view_settings.exposure=.25
    scene.world=bpy.data.worlds.new('Neutral studio'); scene.world.use_nodes=True
    scene.world.node_tree.nodes.get('Background').inputs[0].default_value=(.16,.19,.25,1)
    scene.world.node_tree.nodes.get('Background').inputs[1].default_value=.35
    plate=surface('Saturated background',bottom,1,emission=.45)
    plate_bs=plate.node_tree.nodes.get('Principled BSDF')
    plate_bs.inputs['Specular IOR Level'].default_value=0
    plate_bs.inputs['Coat Weight'].default_value=0
    nodes=plate.node_tree.nodes; links=plate.node_tree.links
    coord=nodes.new('ShaderNodeTexCoord'); split=nodes.new('ShaderNodeSeparateXYZ'); ramp=nodes.new('ShaderNodeValToRGB')
    links.new(coord.outputs['Generated'],split.inputs[0]); links.new(split.outputs['Y'],ramp.inputs[0])
    ramp.color_ramp.elements[0].color=(*rgb(bottom),1); ramp.color_ramp.elements[1].color=(*rgb(top),1)
    for socket in ['Base Color','Emission Color']: links.new(ramp.outputs[0],nodes.get('Principled BSDF').inputs[socket])
    glow=nodes.new('ShaderNodeEmission'); glow.inputs['Strength'].default_value=.7
    links.new(ramp.outputs[0],glow.inputs['Color'])
    blend=nodes.new('ShaderNodeMixShader'); blend.inputs[0].default_value=.85
    links.new(plate_bs.outputs[0],blend.inputs[1]); links.new(glow.outputs[0],blend.inputs[2])
    links.new(blend.outputs[0],nodes.get('Material Output').inputs['Surface'])
    # A flat face with a rounded outline, independent of the object's depth.
    points=[]
    for cx,cy,start in [(2.30,2.30,0),(-2.30,2.30,90),(-2.30,-2.30,180),(2.30,-2.30,270)]:
        for i in range(33):
            a=math.radians(start+i*90/32)
            points.append((cx+1.82*math.cos(a),cy+1.82*math.sin(a)))
    prism('Icon background',points,-.14,.14,plate,'01 background',.015)
    bpy.ops.object.camera_add(location=(0,0,20))
    cam=bpy.context.object; cam.rotation_euler=(0,0,0)
    cam.rotation_euler=(Vector((0,0,0))-cam.location).to_track_quat('-Z','Y').to_euler()
    cam.data.type='ORTHO'; cam.data.ortho_scale=10.24; scene.camera=cam
    for name,loc,power,sx,sy,color in [
        ('Long white reflection',(-3,4,8),1050,5,.85,(1,.96,.90)),
        ('Cool vertical edge',(4,1,6),850,1.2,5,(.68,.83,1)),
        ('Low cyan rim',(-4,-3,4),650,.7,4,(.55,1,.94)),
        ('Bubble highlight',(1.2,2.5,9),180,1,.5,(1,1,1))]:
        bpy.ops.object.light_add(type='AREA',location=loc)
        light=bpy.context.object; light.name=name; light.data.energy=power; light.data.shape='RECTANGLE'
        light.data.size=sx; light.data.size_y=sy; light.data.color=color
        light.rotation_euler=(Vector((0,0,1))-light.location).to_track_quat('-Z','Y').to_euler()
    return scene

def level_markings(material):
    n=material.node_tree.nodes; l=material.node_tree.links
    coord=n.new('ShaderNodeTexCoord'); split=n.new('ShaderNodeSeparateXYZ')
    l.new(coord.outputs['Generated'],split.inputs[0])
    masks=[]
    for center in (.355,.645):
        compare=n.new('ShaderNodeMath'); compare.operation='COMPARE'
        compare.inputs[1].default_value=center; compare.inputs[2].default_value=.014
        l.new(split.outputs['X'],compare.inputs[0]); masks.append(compare)
    maximum=n.new('ShaderNodeMath'); maximum.operation='MAXIMUM'
    for i,mask in enumerate(masks): l.new(mask.outputs[0],maximum.inputs[i])
    ink=n.new('ShaderNodeBsdfPrincipled'); ink.inputs['Base Color'].default_value=(*rgb('063F37'),1)
    ink.inputs['Roughness'].default_value=.24
    mix=n.new('ShaderNodeMixShader'); l.new(maximum.outputs[0],mix.inputs[0])
    l.new(n.get('Principled BSDF').outputs[0],mix.inputs[1]); l.new(ink.outputs[0],mix.inputs[2])
    l.new(mix.outputs[0],n.get('Material Output').inputs['Surface'])

for variant in variants:
    if variant=='A':
        scene=setup('10396E','030C21')
        glass=surface('Optical glass','D8FFF4',.045,1,1.46)
        level_markings(glass)
        shell=rounded_box('Level glass shell',(0,0,1.25),(6.8,2.65,1.85),.86,glass,'02 tube and markings')
        cavity=rounded_box('Tube cavity',(0,0,1.25),(6.45,2.29,1.49),.72,None,'temporary')
        difference(shell,cavity)
        liquid_mat=surface('Emerald liquid','07B780',.035,1,1.333,.12,.45)
        liquid_mat.node_tree.nodes.get('Principled BSDF').inputs['Base Color'].default_value=(*rgb('D0FFF0'),1)
        liquid=rounded_box('Liquid with centered air pocket',(0,0,1.25),(6.39,2.23,1.43),.69,liquid_mat,'03 liquid and bubble')
        bubble=sphere('Air cavity',(0,0,1.39),(.70,.76,.50),None,'temporary')
        difference(liquid,bubble)
        air=surface('Luminous air interface','E9FFF6',.025,.92,1.10,.15)
        sphere('Centered air bubble',(0,0,1.39),(.68,.74,.48),air,'04 air interface')
    elif variant=='B':
        scene=setup('361D52','100B24')
        glass=surface('Frosted blue glass','DBE9FF',.16,.985,1.46,absorption=.035)
        rounded_box('Envelope glass body',(0,-.05,.70),(6.05,4.35,.90),.36,glass,'02 envelope body')
        warm=surface('Warm interior','FFAD48',.25,emission=3.6)
        triangle=[(-2.67,1.58),(0,-.62),(2.67,1.58)]
        core=prism('Internal warm light',triangle,1.17,.035,warm,'03 warm interior',.06)
        inner=[(x*.72,(y-.8467)*.72+.8467) for x,y in triangle]
        cut=prism('Interior light opening',inner,1.0,.5,warm,'temporary',.02)
        difference(core,cut)
        flapglass=surface('Satin glass flap','D6E9FF',.13,.98,1.46,absorption=.018)
        smaller=[(x*.965,y*.965+.018) for x,y in triangle]
        prism('Glass flap',smaller,1.25,.25,flapglass,'04 folded glass',.10)
    else:
        scene=setup('401044','120B25')
        rose=surface('Rose crystal','FF4E94',.045,1,1.46,.18,.15)
        rose.node_tree.nodes.get('Principled BSDF').inputs['Base Color'].default_value=(*rgb('FFEFF7'),1)
        c=bpy.data.curves.new('Open glass orbit','CURVE'); c.dimensions='3D'; c.bevel_depth=.68; c.bevel_resolution=16; c.use_fill_caps=True
        spline=c.splines.new('POLY'); spline.points.add(240)
        for i,p in enumerate(spline.points):
            a=math.radians(70+280*i/240)
            p.co=(2.22*math.cos(a),2.22*math.sin(a),1.02+.20*math.sin(a),1)
        obj=bpy.data.objects.new('Rose glass orbit',c); bpy.context.collection.objects.link(obj)
        obj.data.materials.append(rose); obj['icon_layer']='02 open glass orbit'
        pearl=surface('Quiet pearl','E5FFF9',.035,1,1.46,.015)
        sphere('One clear focus',(.24,.12,1.10),(1.05,1.05,1.05),pearl,'03 focal pearl')
        center=surface('Pearl inner light','FFF1C9',.30,emission=.65)
        sphere('Quiet luminous core',(.24,.12,1.10),(.60,.60,.60),center,'04 pearl light')
    visible=[o for o in scene.objects if o.type in {'MESH','CURVE'} and not o.hide_render]
    assert 3 <= len(visible) <= 4
    assert all('icon_layer' in o for o in visible)
    print('Hero',variant,'layers:',[o['icon_layer'] for o in visible],flush=True)
    scene.render.filepath=str(out/('hero-'+variant+'.png'))
    bpy.ops.render.render(write_still=True)
assert all((out/('hero-'+v+'.png')).exists() for v in variants)
