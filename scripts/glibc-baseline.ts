// Linux release binaries must run on the oldest supported glibc distribution.
// The release flow measures every Linux executable's dynamic symbol versions and
// records the baseline in release.json, so install.sh can refuse an older host
// before it downloads the binary.

/** Highest glibc symbol version a Linux release binary may require (RHEL/Rocky/Alma 9). */
export const LINUX_GLIBC_BASELINE = "2.34";

const SHT_DYNAMIC = 6;
const SHT_GNU_VERNEED = 0x6ffffffe;
const DT_NEEDED = 1n;

/** Dynamic linkage facts read from one ELF64 little-endian executable. */
export interface ElfLinkage {
  /** `DT_NEEDED` shared libraries. */
  needed: string[];
  /** Required symbol versions by shared library file. */
  versions: Map<string, string[]>;
}

function cString(bytes: Buffer, offset: number): string {
  const end = bytes.indexOf(0, offset);
  if (offset < 0 || offset >= bytes.length || end < 0)
    throw new Error("ELF string table entry is out of bounds");
  return bytes.toString("latin1", offset, end);
}

/** Read `DT_NEEDED` and `.gnu.version_r` from an ELF64 little-endian binary. */
export function elfLinkage(bytes: Buffer): ElfLinkage {
  if (
    bytes.length < 64 ||
    bytes.readUInt32BE(0) !== 0x7f454c46 ||
    bytes[4] !== 2 ||
    bytes[5] !== 1
  )
    throw new Error("only ELF64 little-endian Linux binaries are supported");
  const sectionOffset = Number(bytes.readBigUInt64LE(0x28));
  const sectionSize = bytes.readUInt16LE(0x3a);
  const sectionCount = bytes.readUInt16LE(0x3c);
  if (sectionSize !== 64 || sectionOffset + sectionCount * 64 > bytes.length)
    throw new Error("ELF section header table is invalid");
  const sections = Array.from({ length: sectionCount }, (_, index) => {
    const base = sectionOffset + index * 64;
    return {
      type: bytes.readUInt32LE(base + 4),
      offset: Number(bytes.readBigUInt64LE(base + 0x18)),
      size: Number(bytes.readBigUInt64LE(base + 0x20)),
      link: bytes.readUInt32LE(base + 0x28),
      info: bytes.readUInt32LE(base + 0x2c),
    };
  });
  const strings = (link: number): number => {
    const table = sections[link];
    if (table === undefined || table.offset + table.size > bytes.length)
      throw new Error("ELF string table is invalid");
    return table.offset;
  };
  const needed: string[] = [];
  const versions = new Map<string, string[]>();
  for (const section of sections) {
    if (section.type !== SHT_DYNAMIC && section.type !== SHT_GNU_VERNEED)
      continue;
    if (section.offset + section.size > bytes.length)
      throw new Error("ELF section is out of bounds");
    if (section.type === SHT_DYNAMIC) {
      const table = strings(section.link);
      for (let entry = 0; entry + 16 <= section.size; entry += 16) {
        const tag = bytes.readBigInt64LE(section.offset + entry);
        if (tag === 0n) break;
        if (tag === DT_NEEDED)
          needed.push(
            cString(
              bytes,
              table + Number(bytes.readBigUInt64LE(section.offset + entry + 8)),
            ),
          );
      }
    } else if (section.type === SHT_GNU_VERNEED) {
      const table = strings(section.link);
      let need = section.offset;
      for (let count = 0; count < section.info; count += 1) {
        const file = cString(bytes, table + bytes.readUInt32LE(need + 4));
        const names = versions.get(file) ?? [];
        let aux = need + bytes.readUInt32LE(need + 8);
        for (let index = 0; index < bytes.readUInt16LE(need + 2); index += 1) {
          names.push(cString(bytes, table + bytes.readUInt32LE(aux + 8)));
          const next = bytes.readUInt32LE(aux + 12);
          if (next === 0) break;
          aux += next;
        }
        versions.set(file, names);
        const next = bytes.readUInt32LE(need + 12);
        if (next === 0) break;
        need += next;
      }
    }
  }
  return { needed, versions };
}

function compareVersions(left: string, right: string): number {
  const a = left.split(".").map(Number);
  const b = right.split(".").map(Number);
  for (let index = 0; index < Math.max(a.length, b.length); index += 1) {
    const difference = (a[index] ?? 0) - (b[index] ?? 0);
    if (difference !== 0) return difference;
  }
  return 0;
}

/**
 * Return the highest `GLIBC_` version the binary requires, failing when it
 * exceeds the baseline or links the system C++ runtime dynamically.
 */
export function checkGlibcBaseline(
  bytes: Buffer,
  label: string,
  baseline = LINUX_GLIBC_BASELINE,
): string {
  const linkage = elfLinkage(bytes);
  if (linkage.needed.some((library) => library.startsWith("libstdc++.so")))
    throw new Error(`${label} links libstdc++ dynamically`);
  let highest = "0";
  for (const names of linkage.versions.values())
    for (const name of names) {
      if (name.startsWith("GLIBCXX_") || name.startsWith("CXXABI_"))
        throw new Error(`${label} requires system C++ runtime ${name}`);
      if (!name.startsWith("GLIBC_") || !/^GLIBC_\d+(\.\d+)*$/.test(name))
        continue;
      const version = name.slice("GLIBC_".length);
      if (compareVersions(version, highest) > 0) highest = version;
    }
  if (compareVersions(highest, baseline) > 0)
    throw new Error(
      `${label} requires GLIBC_${highest}, above the GLIBC_${baseline} release baseline`,
    );
  return highest;
}
