import hashlib, struct
MAGIC = bytes([0x11,0xf4,0x7a,0xd5,0x7b,0x73,0x42,0xc0,0xab,0xae,0x87,0x8b,0x1e,0x16,0xad,0xee])

def iid(sig: str):
    h = hashlib.sha1(MAGIC + sig.encode('utf-8')).digest()
    d1 = struct.unpack('>I', h[0:4])[0]
    d2 = struct.unpack('>H', h[4:6])[0]
    d3 = struct.unpack('>H', h[6:8])[0]
    d3 = (d3 & 0x0fff) | (5 << 12)
    d4b0 = (h[8] & 0x3f) | 0x80
    d4 = [d4b0] + list(h[9:16])
    return f"{d1:08X}-{d2:04X}-{d3:04X}-" + "".join(f"{b:02X}" for b in d4)

shapes = {
 'IVector': '913337e9-11a1-4345-a3a2-4e7f956e222d',
 'IVectorView': '6a79e863-4300-459a-9966-cbb660963ee1',
 'IIterable': 'faa585ea-6214-4217-afda-7f46de5869b3',
}
args = {
 'cinterface(SetterBase)': 'cinterface(Windows.UI.Xaml.SetterBase)',
 'isetterbase': 'cinterface(Windows.UI.Xaml.ISetterBase)',
 'plain': 'Windows.UI.Xaml.SetterBase',
}
# class signature per windows-rs for_class: rc(name;{default iid}) where default iface of SetterBase is ISetterBase
args['rc(SetterBase;ISetterBase)'] = 'rc(Windows.UI.Xaml.SetterBase;{418be27c-2ac4-4f22-8097-dea3aeeb2fb3})'
args['rc(SetterBase;ISetter)'] = 'rc(Windows.UI.Xaml.SetterBase;{a73ded29-b4ae-4a81-be85-e690ba0d3b6e})'

observed = {
 'iid1': 'BD5ED2BD-7501-5519-9FFBD0074240DB61',
 'iid2': '88F0????', # filled below
}
for sname, sguid in shapes.items():
    for aname, asig in args.items():
        sig = f"pinterface({{{sguid}}};{asig})"
        print(f"{sname:12s} {aname:28s} {iid(sig)}")
