import type { Container as ExperimentalContainer } from "@cloudflare/workers-types/experimental/index.ts";

type NativeContainer = NonNullable<DurableObjectState["container"]>;
const SOURCE = "/tmp/p23-directory-state";
const COPY = "/tmp/p23-directory-copy";
const EMPTY = "/tmp/p23-directory-empty";

function hasCapture(
  value: NativeContainer,
): value is NativeContainer & Pick<ExperimentalContainer, "snapshotDirectory"> {
  return (
    "snapshotDirectory" in value &&
    typeof value.snapshotDirectory === "function"
  );
}

function record(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

export function directoryHandle(value: unknown): ContainerDirectorySnapshot {
  if (
    !record(value) ||
    Object.keys(value).some(
      (key) => !["id", "size", "dir", "name"].includes(key),
    ) ||
    typeof value.id !== "string" ||
    value.id.length === 0 ||
    value.id.length > 256 ||
    value.id.includes("\0") ||
    typeof value.size !== "number" ||
    !Number.isSafeInteger(value.size) ||
    value.size < 0 ||
    typeof value.dir !== "string" ||
    !value.dir.startsWith("/") ||
    value.dir.includes("\0") ||
    (value.name !== undefined && typeof value.name !== "string")
  )
    throw new Error("Invalid directory fixture handle");
  return {
    id: value.id,
    size: value.size,
    dir: value.dir,
    ...(value.name === undefined ? {} : { name: value.name }),
  };
}

async function python(
  native: NativeContainer,
  source: string,
  args: string[] = [],
): Promise<unknown> {
  const process = await native.exec(["python3", "-c", source, ...args], {
    user: "0:0",
    cwd: "/",
  });
  const output = await process.output();
  if (
    output.exitCode !== 0 ||
    output.stderr.byteLength !== 0 ||
    output.stdout.byteLength > 16384
  )
    throw new Error("Directory fixture execution failed");
  const parsed: unknown = JSON.parse(new TextDecoder().decode(output.stdout));
  return parsed;
}

function mounts(
  snapshot: ContainerDirectorySnapshot,
): ContainerDirectorySnapshotRestoreParams[] {
  return [
    { snapshot },
    { snapshot, mountPoint: COPY },
    { mountPoint: EMPTY },
    { mountPoint: `${COPY}/child` },
  ];
}

const WRITE = `import os,json,sys
root=sys.argv[1]; value=sys.argv[2]
os.makedirs(root+'/child',exist_ok=True)
for path,text in [(root+'/state.txt',value),(root+'/child/nested.txt','nested')]:
 with open(path,'w') as output: output.write(text)
 os.chmod(path,0o640); os.chown(path,1001,1002); os.utime(path,(1700000000,1700000000))
for path in [root+'/'+name for name in ['alias','parent-link','deep-link','absolute-link']]:
 if os.path.lexists(path): os.unlink(path)
os.link(root+'/state.txt',root+'/alias')
for name,target in [('parent-link','../elsewhere'),('deep-link','../../../../p23-directory-dangling'),('absolute-link','/p23-directory-dangling')]:
 path=root+'/'+name; os.symlink(target,path); os.lchown(path,1001,1002); os.utime(path,(1700000000,1700000000),follow_symlinks=False)
for path in [root,root+'/child']: os.chmod(path,0o750); os.chown(path,1001,1002); os.utime(path,(1700000000,1700000000))
print(json.dumps({'written':value}))`;
const MUTATE = `import os,json,sys
path=sys.argv[1]+'/state.txt'
with open(path,'w') as output: output.write('mutated')
os.utime(path,(1700000000,1700000000))
print(json.dumps({'written':'mutated'}))`;
const READ = `import os,json,sys,stat
result={}
for root in sys.argv[1:]:
 path=root+'/state.txt'; row=os.stat(path); alias=os.stat(root+'/alias'); link=os.lstat(root+'/parent-link'); directory=os.stat(root)
 with open(path) as source: value=source.read()
 with open(root+'/alias') as source: alias_value=source.read()
 child=root+'/child'
 extra={}
 for name in ['deep-link','absolute-link']:
  path=root+'/'+name; info=os.lstat(path); extra[name]={'target':os.readlink(path),'symlink':stat.S_ISLNK(info.st_mode),'uid':info.st_uid,'gid':info.st_gid,'mtime':int(info.st_mtime)}
 result[root]={'value':value,'aliasValue':alias_value,'mode':stat.S_IMODE(row.st_mode),'uid':row.st_uid,'gid':row.st_gid,'mtime':int(row.st_mtime),'hardlink':row.st_ino==alias.st_ino and row.st_dev==alias.st_dev and row.st_nlink>=2,'symlink':stat.S_ISLNK(link.st_mode),'symlinkTarget':os.readlink(root+'/parent-link'),'symlinkMetadata':{'uid':link.st_uid,'gid':link.st_gid,'mtime':int(link.st_mtime)},'directoryMetadata':{'mode':stat.S_IMODE(directory.st_mode),'uid':directory.st_uid,'gid':directory.st_gid,'mtime':int(directory.st_mtime)},'childFiles':sorted(os.listdir(child))}
 result[root]['additionalLinks']=extra
result['empty']=sorted(os.listdir('/tmp/p23-directory-empty')) if os.path.isdir('/tmp/p23-directory-empty') else None
print(json.dumps(result))`;

/** Test-only routes use the actual native Container; handles come from capture responses. */
export async function directoryAction(
  native: NativeContainer,
  baseImage: string | undefined,
  request: Request,
): Promise<Response> {
  const url = new URL(request.url);
  const action = url.pathname;
  const startup: ContainerStartupOptions =
    baseImage === undefined
      ? { enableInternet: false }
      : { image: baseImage, enableInternet: false, instance: "lite" };
  if (action === "/named/directory/surface")
    return Response.json({ capture: hasCapture(native) });
  if (action === "/named/directory/stop") {
    await native.destroy();
    return Response.json({ running: native.running });
  }
  if (
    action === "/named/directory/restore" ||
    action === "/named/directory/replay"
  ) {
    if (request.method !== "POST")
      return new Response("POST required", { status: 405 });
    const body: unknown = await request.json();
    const snapshot = directoryHandle(body);
    if (action.endsWith("/restore") && native.running)
      throw new Error("Directory restore requires a stopped fixture");
    if (action.endsWith("/replay") && !native.running)
      throw new Error("Directory replay requires a running fixture");
    const replay = action.endsWith("/replay");
    let startRejected = false;
    try {
      native.start({ ...startup, directorySnapshots: mounts(snapshot) });
    } catch (error) {
      if (!replay || !(error instanceof Error)) throw error;
      startRejected = true;
    }
    if (replay && !startRejected)
      throw new Error("Running fixture start must reject");
    if (!replay)
      // Native startup can outlast the header deadline; the original body deadline still applies.
      return new Response(
        new ReadableStream<Uint8Array>({
          async start(controller) {
            try {
              const result = await python(native, READ, [SOURCE, COPY]);
              controller.enqueue(
                new TextEncoder().encode(JSON.stringify(result)),
              );
              controller.close();
            } catch {
              controller.error(new Error("Directory fixture restore failed"));
            }
          },
        }),
        { headers: { "Content-Type": "application/json" } },
      );
    const result = await python(native, READ, [SOURCE, COPY]);
    if (!record(result)) throw new Error("Invalid directory fixture response");
    return Response.json({ ...result, startRejected });
  }
  if (action === "/named/directory/read")
    return Response.json(
      await python(
        native,
        READ,
        url.searchParams.get("source") === "1" ? [SOURCE] : [SOURCE, COPY],
      ),
    );
  if (action === "/named/directory/mutate")
    return Response.json(await python(native, MUTATE, [SOURCE]));
  if (action === "/named/directory/write") {
    if (!native.running) native.start(startup);
    return Response.json(await python(native, WRITE, [SOURCE, "saved"]));
  }
  if (action === "/named/directory/capture") {
    if (!hasCapture(native))
      throw new Error("Native directory capture is required");
    const capture = native.snapshotDirectory.bind(native);
    const options =
      url.searchParams.get("unnamed") === "1"
        ? { dir: "//tmp/./p23-directory-state/" }
        : { dir: "//tmp/./p23-directory-state/", name: "directory-checkpoint" };
    // Send headers first; the original response deadline still governs this stream.
    return new Response(
      new ReadableStream<Uint8Array>({
        async start(controller) {
          try {
            const snapshot = await capture(options);
            controller.enqueue(
              new TextEncoder().encode(JSON.stringify(snapshot)),
            );
            controller.close();
          } catch {
            controller.error(new Error("Directory fixture capture failed"));
          }
        },
      }),
      { headers: { "Content-Type": "application/json" } },
    );
  }
  return new Response("unknown directory action", { status: 404 });
}
