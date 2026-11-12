"""Read PE import metadata and ask the Windows loader to resolve each dependency."""
import ctypes
from ctypes import wintypes
import json
import pathlib
import struct
import sys

binary = pathlib.Path(sys.argv[1]).resolve(strict=True)
data = binary.read_bytes()
u16 = lambda p: struct.unpack_from('<H', data, p)[0]
u32 = lambda p: struct.unpack_from('<I', data, p)[0]
pe = u32(0x3c)
assert data[pe:pe+4] == b'PE\0\0'
sections_count, optional_size = u16(pe+6), u16(pe+20)
optional = pe+24
is_64 = u16(optional) == 0x20b
directory = optional + (112 if is_64 else 96)
import_rva, import_size = u32(directory+8), u32(directory+12)
sections = []
for i in range(sections_count):
    p = optional + optional_size + 40*i
    sections.append((u32(p+12), max(u32(p+8), u32(p+16)), u32(p+20)))
def offset(rva):
    for base, size, raw in sections:
        if base <= rva < base+size:
            return raw + rva-base
    if rva < u32(optional+60):
        return rva
    raise ValueError(f'Unmapped RVA {rva:#x}')
def cstring(rva):
    start = offset(rva)
    return data[start:data.index(b'\0', start)].decode('ascii')

kernel = ctypes.WinDLL('kernel32', use_last_error=True)
kernel.SetErrorMode(3)
kernel.LoadLibraryExW.argtypes = [wintypes.LPCWSTR, wintypes.HANDLE, wintypes.DWORD]
kernel.LoadLibraryExW.restype = wintypes.HMODULE
kernel.GetProcAddress.argtypes = [wintypes.HMODULE, ctypes.c_void_p]
kernel.GetProcAddress.restype = ctypes.c_void_p
kernel.GetModuleFileNameW.argtypes = [wintypes.HMODULE, wintypes.LPWSTR, wintypes.DWORD]
kernel.GetModuleFileNameW.restype = wintypes.DWORD
kernel.FreeLibrary.argtypes = [wintypes.HMODULE]
kernel.FreeLibrary.restype = wintypes.BOOL

reports = []
index = 0
while True:
    p = offset(import_rva)+index*20
    original, stamp, chain, name_rva, first = struct.unpack_from('<IIIII', data, p)
    if not any((original, stamp, chain, name_rva, first)):
        break
    dll = cstring(name_rva)
    module = kernel.LoadLibraryExW(dll, None, 0)
    row = {'dll': dll, 'loaded': bool(module), 'missing': [], 'imported_symbols': []}
    if not module:
        row['error'] = ctypes.get_last_error()
    else:
        path = ctypes.create_unicode_buffer(32768)
        kernel.GetModuleFileNameW(module, path, len(path))
        row['resolved'] = path.value
        table = offset(original or first)
        size, fmt, flag = (8, '<Q', 1 << 63) if is_64 else (4, '<I', 1 << 31)
        j = 0
        while True:
            thunk = struct.unpack_from(fmt, data, table+size*j)[0]
            if not thunk:
                break
            if thunk & flag:
                symbol = thunk & 0xffff
                pointer = ctypes.c_void_p(symbol)
            else:
                symbol = cstring(thunk+2)
                encoded = symbol.encode('ascii')
                pointer = ctypes.cast(ctypes.c_char_p(encoded), ctypes.c_void_p)
            row['imported_symbols'].append(symbol)
            if not kernel.GetProcAddress(module, pointer):
                row['missing'].append(symbol)
            j += 1
        row['symbols'] = j
        kernel.FreeLibrary(module)
    reports.append(row)
    index += 1
def exported_names(path):
    blob = pathlib.Path(path).read_bytes()
    word = lambda p: struct.unpack_from('<H', blob, p)[0]
    dword = lambda p: struct.unpack_from('<I', blob, p)[0]
    header = dword(0x3c)
    opt = header+24
    directories = opt+(112 if word(opt) == 0x20b else 96)
    regions = []
    for i in range(word(header+6)):
        p = opt+word(header+20)+40*i
        regions.append((dword(p+12), max(dword(p+8), dword(p+16)), dword(p+20)))
    def raw(rva):
        for base, length, start in regions:
            if base <= rva < base+length:
                return start+rva-base
        return rva
    table = raw(dword(directories))
    names = raw(dword(table+32))
    result = set()
    for i in range(dword(table+24)):
        start = raw(dword(names+4*i))
        result.add(blob[start:blob.index(b'\0', start)].decode('ascii'))
    return result

common_controls = next(r for r in reports if r['dll'].lower() == 'comctl32.dll')
static_path = pathlib.Path('C:/Windows/System32/comctl32.dll')
static_names = exported_names(static_path)
common_controls['system32_static_path'] = str(static_path)
common_controls['system32_static_missing'] = [n for n in common_controls['imported_symbols'] if isinstance(n, str) and n not in static_names]
resource_rva = u32(directory+16)
resource_base = offset(resource_rva) if resource_rva else None
manifests = []
if resource_base is not None:
    def resources(relative, ids, depth):
        if depth > 3:
            return
        root = resource_base+relative
        count = u16(root+12)+u16(root+14)
        for i in range(count):
            name, dest = struct.unpack_from('<II', data, root+16+8*i)
            current_ids = ids+[name]
            if depth == 0 and name != 24:
                continue
            if dest & 0x80000000:
                resources(dest & 0x7fffffff, current_ids, depth+1)
            else:
                p = resource_base+dest
                start, length = u32(p), u32(p+4)
                manifests.append({'ids': current_ids, 'xml': data[offset(start):offset(start)+length].decode('utf-8', errors='replace')})
    resources(0, [], 0)
for row in reports:
    if row['dll'].lower() != 'comctl32.dll':
        row.pop('imported_symbols', None)
print(json.dumps({'binary': str(binary), 'reports': reports, 'manifests': manifests}, ensure_ascii=False, indent=2))
sys.exit(1 if any(not r['loaded'] or r['missing'] for r in reports) else 0)
