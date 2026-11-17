import sys, zlib, struct
src, dst = sys.argv[1], sys.argv[2]
data = open(src, 'rb').read()
nl = data.index(b'\n')
w, h = map(int, data[:nl].split())
rgba = data[nl+1:]
raw = b''.join(b'\x00' + rgba[y*w*4:(y+1)*w*4] for y in range(h))
def chunk(t, d):
    c = struct.pack('>I', len(d)) + t + d
    return c + struct.pack('>I', zlib.crc32(t + d) & 0xffffffff)
png = b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', w, h, 8, 6, 0, 0, 0)) + chunk(b'IDAT', zlib.compress(raw, 6)) + chunk(b'IEND', b'')
open(dst, 'wb').write(png)
print(w, h)
