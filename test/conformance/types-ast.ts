import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { API, DiagnosticCategory } from "typescript/unstable/async";
import { createVirtualFileSystem } from "typescript/unstable/fs";
import {
  formatSyntaxKind,
  isBigIntLiteral,
  isClassDeclaration,
  isIdentifier,
  isInterfaceDeclaration,
  isModuleDeclaration,
  isNoSubstitutionTemplateLiteral,
  isNumericLiteral,
  isPrivateIdentifier,
  isRegularExpressionLiteral,
  isSourceFile,
  isStringLiteral,
  isTypeAliasDeclaration,
  type Node,
  type SourceFile,
} from "typescript/unstable/ast";

const VIRTUAL_ROOT = "/open-compute-types-ast";
const VIRTUAL_FILE = `${VIRTUAL_ROOT}/index.d.ts`;
const VIRTUAL_TSCONFIG = `${VIRTUAL_ROOT}/tsconfig.json`;

export interface CanonicalNode {
  kind: string;
  text?: string;
  children?: CanonicalNode[];
}

export interface DeclarationFingerprint {
  canonical: string;
  sha256: string;
  statements: number;
  lines: number;
}

function nodeText(node: Node): string | undefined {
  if (
    isIdentifier(node)
    || isPrivateIdentifier(node)
    || isStringLiteral(node)
    || isNumericLiteral(node)
    || isBigIntLiteral(node)
    || isNoSubstitutionTemplateLiteral(node)
    || isRegularExpressionLiteral(node)
  ) {
    return node.text;
  }
  return undefined;
}

export function canonicalize(node: Node): CanonicalNode {
  const children: CanonicalNode[] = [];
  node.forEachChild(child => {
    children.push(canonicalize(child));
  });
  const result: CanonicalNode = { kind: formatSyntaxKind(node.kind) };
  const text = nodeText(node);
  if (text !== undefined) result.text = text;
  if (isSourceFile(node)) {
    const directives = node.typeReferenceDirectives.map(directive => ({
      kind: "TypeReferenceDirective",
      text: directive.fileName,
    }));
    result.children = directives.length ? [...directives, ...children] : children;
    return result;
  }
  if (children.length) result.children = children;
  return result;
}

export async function parseSourceFile(sourceText: string): Promise<SourceFile> {
  const api = new API({
    cwd: VIRTUAL_ROOT,
    fs: createVirtualFileSystem({
      [VIRTUAL_FILE]: sourceText,
      [VIRTUAL_TSCONFIG]: `${JSON.stringify({
        files: ["index.d.ts"],
        compilerOptions: {
          noEmit: true,
          skipLibCheck: true,
          types: [],
          lib: [],
          strict: true,
        },
      })}\n`,
    }),
  });
  try {
    const snapshot = await api.updateSnapshot({ openProjects: [VIRTUAL_TSCONFIG] });
    const project = snapshot.getProjects()[0];
    if (project === undefined) throw new Error("TypeScript 7 did not open a project for the declaration source");
    const diagnostics = [
      ...await project.program.getSyntacticDiagnostics(),
      ...await project.program.getBindDiagnostics(),
    ].filter(diagnostic => diagnostic.category === DiagnosticCategory.Error);
    if (diagnostics.length) {
      throw new Error(`declaration parse failed: ${diagnostics.map(diagnostic => diagnostic.text).join("; ")}`);
    }
    const sourceFile = await project.program.getSourceFile(VIRTUAL_FILE);
    if (sourceFile === undefined) throw new Error("TypeScript 7 did not return a SourceFile");
    return sourceFile;
  } finally {
    await api.close();
  }
}

export async function fingerprintDeclarationSource(sourceText: string): Promise<DeclarationFingerprint> {
  const sourceFile = await parseSourceFile(sourceText);
  const canonical = `${JSON.stringify(canonicalize(sourceFile))}\n`;
  return {
    canonical,
    sha256: createHash("sha256").update(canonical).digest("hex"),
    statements: sourceFile.statements.length,
    lines: sourceText.split(/\n/).length - (sourceText.endsWith("\n") ? 1 : 0),
  };
}

export async function fingerprintDeclarationSourceTwice(sourceText: string): Promise<DeclarationFingerprint> {
  const first = await fingerprintDeclarationSource(sourceText);
  const second = await fingerprintDeclarationSource(sourceText);
  if (first.sha256 !== second.sha256 || first.canonical !== second.canonical) {
    throw new Error("declaration AST extraction is not deterministic");
  }
  return first;
}

export async function assertOpenComputeTypesAreThinBridge(sourceText: string): Promise<void> {
  const sourceFile = await parseSourceFile(sourceText);
  const references = sourceFile.typeReferenceDirectives.map(directive => directive.fileName);
  if (references.length !== 1 || references[0] !== "@cloudflare/workers-types") {
    throw new Error("packages/types must reference only @cloudflare/workers-types");
  }
  for (const statement of sourceFile.statements) {
    if (isInterfaceDeclaration(statement) || isClassDeclaration(statement) || isTypeAliasDeclaration(statement)) {
      throw new Error("packages/types must not declare Cloudflare or Web API types");
    }
    if (!isModuleDeclaration(statement) || !isStringLiteral(statement.name)
        || !statement.name.text.startsWith("open-compute:")) {
      throw new Error("packages/types may only declare open-compute:* modules");
    }
  }
}

async function main(args: string[]): Promise<void> {
  const [command, path] = args;
  if ((command !== "fingerprint" && command !== "thin-bridge") || path === undefined) {
    throw new Error("usage: types-ast.ts fingerprint|thin-bridge <file>");
  }
  const sourceText = await readFile(path, "utf8");
  if (command === "thin-bridge") {
    await assertOpenComputeTypesAreThinBridge(sourceText);
    process.stdout.write(`${JSON.stringify({ status: "ok" })}\n`);
    return;
  }
  const fingerprint = await fingerprintDeclarationSourceTwice(sourceText);
  process.stdout.write(`${JSON.stringify({
    sha256: fingerprint.sha256,
    statements: fingerprint.statements,
    lines: fingerprint.lines,
  })}\n`);
}

const entry = process.argv[1];
if (entry !== undefined && import.meta.url === pathToFileURL(resolve(entry)).href) {
  await main(process.argv.slice(2));
}
