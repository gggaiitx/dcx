using System;
using System.Collections.Immutable;
using System.Reflection.Metadata;

// Minimal signature type provider: renders WinRT signatures as readable strings.
// Needed because the ABI type of a property getter decides whether the returned
// pointer is an interface (call the vtable directly) or a class (QI first).
sealed class SigProvider : ISignatureTypeProvider<string, object>
{
    readonly MetadataReader R;
    public SigProvider(MetadataReader r) { R = r; }

    public string GetArrayType(string elementType, ArrayShape shape) => elementType + "[]";
    public string GetByReferenceType(string elementType) => elementType + "&";
    public string GetFunctionPointerType(MethodSignature<string> signature) => "fnptr";
    public string GetGenericInstantiation(string genericType, ImmutableArray<string> typeArguments)
        => genericType + "<" + string.Join(",", typeArguments) + ">";
    public string GetGenericMethodParameter(object genericContext, int index) => "!!" + index;
    public string GetGenericTypeParameter(object genericContext, int index) => "!" + index;
    public string GetModifiedType(string modifier, string unmodifiedType, bool isRequired) => unmodifiedType;
    public string GetPinnedType(string elementType) => elementType;
    public string GetPointerType(string elementType) => elementType + "*";
    public string GetPrimitiveType(PrimitiveTypeCode typeCode) => typeCode.ToString();
    public string GetSZArrayType(string elementType) => elementType + "[]";

    public string GetTypeFromDefinition(MetadataReader reader, TypeDefinitionHandle handle, byte rawTypeKind)
    {
        var td = reader.GetTypeDefinition(handle);
        return Qualify(reader.GetString(td.Namespace), reader.GetString(td.Name));
    }

    public string GetTypeFromReference(MetadataReader reader, TypeReferenceHandle handle, byte rawTypeKind)
    {
        var tr = reader.GetTypeReference(handle);
        return Qualify(reader.GetString(tr.Namespace), reader.GetString(tr.Name));
    }

    public string GetTypeFromSpecification(MetadataReader reader, object genericContext, TypeSpecificationHandle handle, byte rawTypeKind)
    {
        var ts = reader.GetTypeSpecification(handle);
        return ts.DecodeSignature(this, genericContext);
    }

    static string Qualify(string ns, string name) => string.IsNullOrEmpty(ns) ? name : ns + "." + name;
}
