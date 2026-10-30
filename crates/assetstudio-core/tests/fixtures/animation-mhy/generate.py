from pathlib import Path
import struct,json,hashlib,zlib
OUT=Path(__file__).resolve().parent
class W:
 def __init__(self): self.b=bytearray()
 def raw(self,b):self.b.extend(b)
 def pad(self,n=4):self.b.extend(bytes((-len(self.b))%n))
 def u16(self,x):self.raw(struct.pack('<H',x))
 def u32(self,x):self.raw(struct.pack('<I',x))
 def i32(self,x):self.raw(struct.pack('<i',x))
 def i64(self,x):self.raw(struct.pack('<q',x))
 def f(self,*x):self.raw(struct.pack('<'+'f'*len(x),*x))
 def string(self,x):self.i32(len(x.encode()));self.raw(x.encode());self.pad()
 def ptr(self,x):self.i32(0);self.i64(x)
 def floats(self,seq):self.i32(len(seq));self.f(*seq)
 def xform(self):self.f(0,0,0,0,0,0,1,1,1,1)
 def hand(self):self.xform();self.floats([]);self.f(0,0,0,0)
def clip(acl,curve_count,bones=1,mixed=False):
 w=W();w.string('Verified MHY');w.raw(bytes([0,0,1]));w.pad()
 for _ in range(7):w.i32(0)
 w.f(30);w.i32(0);w.f(0,0,0,0,0,0)
 size_at=len(w.b);w.u32(0);muscle_start=len(w.b)
 # Original HumanPose, four XForms, and average speed (2022.3 player).
 w.xform();w.f(*([0]*7));w.i32(0);w.hand();w.hand();w.floats([]);w.i32(0)
 for _ in range(4):w.xform()
 w.f(0,0,0)
 # StreamedClip: empty word array and the two u16 count halves.
 w.i32(0);w.u16(0);w.u16(0)
 # DenseClip and ConstantClip precede MHY ACL for non-SR profiles.
 w.i32(2 if mixed else 0);w.u32(3 if mixed else 0);w.f(30,0)
 w.floats([10,20,30,40,50,60] if mixed else [])
 w.floats([2,3,4] if mixed else [])
 # Unchanged MHYACLClip.Read: byte count, payload, alignment, curve count.
 w.i32(len(acl));w.raw(acl);w.pad();w.u32(curve_count)
 w.f(0,10/30,0,0,0,0);w.i32(0);w.i32(0);w.floats([]);w.raw(bytes(11));w.pad()
 struct.pack_into('<I',w.b,size_at,len(w.b)-muscle_start)
 bindings=[]
 paths=[0,zlib.crc32(b'Child'),zlib.crc32(b'Child/Leaf')]
 for bone in range(bones):
  for attr in [2,1,3]:bindings.append((paths[bone],attr))
 if mixed:bindings += [(zlib.crc32(b'Child'),1),(zlib.crc32(b'Child'),3)]
 w.i32(len(bindings))
 for path,attr in bindings:
  w.u32(path);w.u32(attr);w.ptr(0);w.i32(4);w.raw(bytes(4))
 w.i32(0);w.raw(bytes(2));w.pad();w.i32(0);w.pad()
 return bytes(w.b),bindings
def game_object(name,components):
 w=W();w.i32(len(components))
 for x in components:w.ptr(x)
 w.i32(0);w.string(name);return bytes(w.b)
def transform(owner,parent,children):
 w=W();w.ptr(owner);w.f(0,0,0,1);w.f(0,0,0);w.f(1,1,1);w.i32(len(children))
 for x in children:w.ptr(x)
 w.ptr(parent);return bytes(w.b)
def animation():
 w=W();w.ptr(1);w.raw(bytes([1]));w.pad();w.ptr(91);w.i32(1);w.ptr(91);return bytes(w.b)
def serialized(objects):
 classes=list(dict.fromkeys(x[0] for x in objects));meta=W();meta.raw(b'2022.3.62f1\0');meta.i32(-2);meta.raw(bytes(1));meta.i32(len(classes))
 for cls in classes:meta.i32(cls);meta.raw(bytes(1));meta.raw(struct.pack('<h',-1));meta.raw(bytes(16))
 meta.i32(len(objects));data=W()
 for cls,pid,payload in objects:
  data.pad()
  while (48+len(meta.b))%4:meta.raw(bytes(1))
  meta.i64(pid);meta.i64(len(data.b));meta.u32(len(payload));meta.i32(classes.index(cls));data.raw(payload)
 meta.raw(bytes(13));offset=(48+len(meta.b)+15)//16*16;header=bytearray(48)
 struct.pack_into('>I',header,8,22);struct.pack_into('>I',header,20,len(meta.b));struct.pack_into('>Q',header,24,offset+len(data.b));struct.pack_into('>Q',header,32,offset)
 return bytes(header)+bytes(meta.b)+bytes(offset-48-len(meta.b))+bytes(data.b)
def scene(payload,three=False):
 return [(1,1,game_object('Root',[11,81])),(4,11,transform(1,0,[12])),(1,2,game_object('Child',[12])),(4,12,transform(2,11,[13] if three else []))]+([(1,3,game_object('Leaf',[13])),(4,13,transform(3,12,[]))]if three else [])+[(111,81,animation()),(74,91,payload)]
manifest={}
for name,source,bones,mixed in [('mhy_variable','animated_variable',1,False),('mhy_mixed_tracks','animated_variable',1,True),('mhy_multibone','mixed_variable',3,False)]:
 acl=(OUT/f'{name}.acl').read_bytes()
 payload,bindings=clip(acl,10*bones,bones,mixed)
 objects=scene(payload,bones==3) if mixed or bones==3 else [(74,91,payload)]
 raw=serialized(objects);path=OUT/(name+'.assets');path.write_bytes(raw)
 (OUT/(name+'.object')).write_bytes(payload)
 manifest[name]={'source_acl':name+'.acl','source_acl_sha256':hashlib.sha256(acl).hexdigest(),'source_csv':name+'.cpp.csv','source_cpp':'ACL v1.1.0 unmodified compressor/decoder; baseline x64 acl.dll independently agrees','format':22,'platform':-2,'class_id':74,'path_id':91,'curve_count':10*bones,'binding_order':[{'path':p,'attribute':a,'components':4 if a==2 else 3} for p,a in bindings],'dense':[10,20,30,40,50,60]if mixed else [],'constants':[2,3,4]if mixed else [],'bytes':len(raw),'sha256':hashlib.sha256(raw).hexdigest(),'notes':'Synthetic complete Unity serialized player bytes in baseline C# order. Non-SR MHY ACL follows ConstantClip and precedes muscle tail; no live game data.'}
(OUT/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
for name,entry in manifest.items():print(name,entry['bytes'],entry['sha256'])
