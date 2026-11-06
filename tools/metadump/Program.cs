using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Reflection.Metadata;
using System.Reflection.PortableExecutable;

// Dumps WinRT interface IIDs and absolute vtable slot indices from a .winmd.
// WinRT ABI slot = 6 (IUnknown 3 + IInspectable 3) + method index within the
// interface. WinMD interfaces all derive from IInspectable, so the first own
// method lands at absolute vtable slot 6. (IUnknown-based COM interfaces such as
// xamlom.h's IXamlDiagnostics are NOT in winmd and start at 3 instead.)
// Used to generate exact Rust constants for dcx-engine; never guess a slot.

class Program
{
    static MetadataReader R;
    static Dictionary<string, TypeDefinitionHandle> Types = new();

    static void Main(string[] args)
    {
        string path = args.Length > 0 ? args[0] : @"C:\Windows\System32\WinMetadata\Windows.UI.Xaml.winmd";
        using var fs = File.OpenRead(path);
        using var pe = new PEReader(fs);
        R = pe.GetMetadataReader();

        foreach (var h in R.TypeDefinitions)
        {
            var td = R.GetTypeDefinition(h);
            string ns = R.GetString(td.Namespace);
            string nm = R.GetString(td.Name);
            string full = string.IsNullOrEmpty(ns) ? nm : ns + "." + nm;
            Types[full] = h;
        }

        // mode 1: dump a whole interface
        if (args.Length >= 3 && args[1] == "--iface")
        {
            DumpInterface(args[2]);
            return;
        }

        // mode 2: resolve (type, property) -> statics IID + slot
        if (args.Length >= 3 && args[1] == "--prop")
        {
            for (int i = 2; i + 1 < args.Length; i += 2)
                ResolveProperty(args[i], args[i + 1]);
            return;
        }

        if (args.Length >= 3 && args[1] == "--guidhex")
        {
            if (!Types.TryGetValue(args[2], out var th)) { Console.WriteLine("not found"); return; }
            var td0 = R.GetTypeDefinition(th);
            foreach (var cah in td0.GetCustomAttributes())
            {
                var ca0 = R.GetCustomAttribute(cah);
                string nm = "?";
                if (ca0.Constructor.Kind == HandleKind.MemberReference)
                {
                    var mr0 = R.GetMemberReference((MemberReferenceHandle)ca0.Constructor);
                    if (mr0.Parent.Kind == HandleKind.TypeReference)
                        nm = R.GetString(R.GetTypeReference((TypeReferenceHandle)mr0.Parent).Name);
                }
                var br = R.GetBlobReader(ca0.Value);
                var bytes = br.ReadBytes(br.Length);
                Console.WriteLine($"{nm}: {BitConverter.ToString(bytes)}");
            }
            return;
        }

        // mode 4: dump a runtime class: implemented interfaces, default interface,
        // activation factory, statics.
        if (args.Length >= 3 && args[1] == "--class")
        {
            for (int i = 2; i < args.Length; i++) DumpClass(args[i]);
            return;
        }

        // mode 5: dump enum values.
        if (args.Length >= 3 && args[1] == "--enum")
        {
            for (int i = 2; i < args.Length; i++) DumpEnum(args[i]);
            return;
        }

        Console.WriteLine("usage: metadump <winmd> --iface <Full.Type.Name>");
        Console.WriteLine("       metadump <winmd> --prop <Type> <Prop> [<Type> <Prop> ...]");
        Console.WriteLine("       metadump <winmd> --class <Full.Type.Name> [...]");
        Console.WriteLine("       metadump <winmd> --enum <Full.Type.Name> [...]");
    }

    static string TypeFullName(EntityHandle h)
    {
        if (h.Kind == HandleKind.TypeReference)
        {
            var tr = R.GetTypeReference((TypeReferenceHandle)h);
            return Qualify(R.GetString(tr.Namespace), R.GetString(tr.Name));
        }
        if (h.Kind == HandleKind.TypeDefinition)
        {
            var td = R.GetTypeDefinition((TypeDefinitionHandle)h);
            return Qualify(R.GetString(td.Namespace), R.GetString(td.Name));
        }
        return "?";
    }

    static string Qualify(string ns, string name) => string.IsNullOrEmpty(ns) ? name : ns + "." + name;

    static void DumpClass(string full)
    {
        if (!Types.TryGetValue(full, out var h)) { Console.WriteLine($"!! not found: {full}"); return; }
        var td = R.GetTypeDefinition(h);
        Console.WriteLine($"=== class {full}  (IID {GetGuid(td):D})");
        if (td.BaseType.Kind != HandleKind.TypeReference)
            Console.WriteLine("    (no base type)");
        else
            Console.WriteLine($"    base {TypeFullName(td.BaseType)}");

        // default interface: DefaultAttribute value is a Type.
        foreach (var cah in td.GetCustomAttributes())
        {
            var ca = R.GetCustomAttribute(cah);
            if (ca.Constructor.Kind != HandleKind.MemberReference) continue;
            var mr = R.GetMemberReference((MemberReferenceHandle)ca.Constructor);
            if (mr.Parent.Kind != HandleKind.TypeReference) continue;
            var tr = R.GetTypeReference((TypeReferenceHandle)mr.Parent);
            string tn = R.GetString(tr.Name);
            if (tn != "DefaultAttribute" && tn != "ActivatableAttribute" &&
                tn != "StaticAttribute" && tn != "ComposableAttribute") continue;
            var br = R.GetBlobReader(ca.Value);
            br.ReadUInt16(); // prolog
            try
            {
                string target = DecodeTypeArg(ref br);
                Console.WriteLine($"    {tn,-22} -> {target}");
            }
            catch { Console.WriteLine($"    {tn,-22} -> (undecodable)"); }
        }

        int ii = 0;
        foreach (var ih in td.GetInterfaceImplementations())
        {
            var impl = R.GetInterfaceImplementation(ih);
            string name = TypeFullName(impl.Interface);
            if (impl.Interface.Kind == HandleKind.TypeSpecification)
            {
                var ts = R.GetTypeSpecification((TypeSpecificationHandle)impl.Interface);
                try { name = ts.DecodeSignature(new SigProvider(R), null); } catch { name = "?"; }
            }
            if (ii++ < 64) Console.WriteLine($"    iface  {name}");
        }
    }

    static string DecodeTypeArg(ref BlobReader br)
    {
        byte code = br.ReadByte();
        if (code == 0x12) // ELEMENT_TYPE_CLASS
        {
            var h = br.ReadTypeHandle();
            return TypeFullName(h);
        }
        if (code == 0x0e) return "string";
        if (code == 0x08) return "int32";
        return "0x" + code.ToString("X2");
    }

    static void DumpEnum(string full)
    {
        if (!Types.TryGetValue(full, out var h)) { Console.WriteLine($"!! not found: {full}"); return; }
        var td = R.GetTypeDefinition(h);
        Console.WriteLine($"=== enum {full}");
        foreach (var fh in td.GetFields())
        {
            var fd = R.GetFieldDefinition(fh);
            if ((fd.Attributes & System.Reflection.FieldAttributes.Static) == 0) continue;
            string val = "?";
            try
            {
                var c = fd.GetDefaultValue();
                if (!c.IsNil)
                {
                    var br2 = R.GetBlobReader(R.GetConstant(c).Value);
                    val = br2.ReadInt32().ToString();
                }
            }
            catch { }
            Console.WriteLine($"    {R.GetString(fd.Name),-24} = {val}");
        }
    }

    static void DumpInterface(string full)
    {
        if (!Types.TryGetValue(full, out var h)) { Console.WriteLine($"!! not found: {full}"); return; }
        var td = R.GetTypeDefinition(h);
        var provider = new SigProvider(R);
        Console.WriteLine($"=== {full}");
        Console.WriteLine($"    IID {GetGuid(td):D}");
        int idx = 0;
        foreach (var mh in td.GetMethods())
        {
            var md = R.GetMethodDefinition(mh);
            string sig = "";
            try
            {
                var ms = md.DecodeSignature(provider, null);
                sig = $" : {ms.ReturnType} ({string.Join(", ", ms.ParameterTypes)})";
            }
            catch { }
            Console.WriteLine($"    slot {6 + idx,-3} {R.GetString(md.Name)}{sig}");
            idx++;
        }
    }

    static void ResolveProperty(string typeFull, string prop)
    {
        // The DP getter is a static method named get_<prop>Property, living on
        // an I<Type>Statics / I<Type>Statics2..N interface.
        string want = "get_" + prop + "Property";
        string shortName = typeFull.Split('.').Last();

        // Statics live on interfaces named I<Type>Statics, I<Type>Statics2, ...
        var candidates = Types.Keys
            .Where(k => k.Split('.').Last().StartsWith("I" + shortName + "Statics"))
            .OrderBy(k => k.Length)
            .ThenBy(k => k)
            .ToList();

        bool found = false;
        foreach (var cand in candidates)
        {
            var td = R.GetTypeDefinition(Types[cand]);
            int idx = 0;
            foreach (var mh in td.GetMethods())
            {
                var md = R.GetMethodDefinition(mh);
                if (R.GetString(md.Name) == want)
                {
                    Console.WriteLine($"{typeFull}.{prop,-22} -> {cand}");
                    Console.WriteLine($"    IID  {GetGuid(td):D}");
                    Console.WriteLine($"    slot {6 + idx}");
                    found = true;
                }
                idx++;
            }
        }
        if (!found) Console.WriteLine($"{typeFull}.{prop,-22} -> NOT FOUND (candidates: {candidates.Count})");
    }

    static Guid GetGuid(TypeDefinition td)
    {
        foreach (var cah in td.GetCustomAttributes())
        {
            var ca = R.GetCustomAttribute(cah);
            if (ca.Constructor.Kind != HandleKind.MemberReference) continue;
            var mr = R.GetMemberReference((MemberReferenceHandle)ca.Constructor);
            if (mr.Parent.Kind != HandleKind.TypeReference) continue;
            var tr = R.GetTypeReference((TypeReferenceHandle)mr.Parent);
            string tn = R.GetString(tr.Name);
            if (tn != "GuidAttribute") continue;

            // WinMD stores the GUID as 16 raw bytes (little-endian for the
            // first three fields), not as a string like System.GuidAttribute.
            var blob = R.GetBlobReader(ca.Value);
            blob.ReadUInt16(); // prolog
            var raw = blob.ReadBytes(16);
            return new Guid(raw);
        }
        return Guid.Empty;
    }
}
