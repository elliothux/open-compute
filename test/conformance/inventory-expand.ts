/** Deterministic AST expansion of public members from pinned declaration nodes. */

import {
  ModifierFlags,
  SyntaxKind,
  isCallSignatureDeclaration,
  isClassDeclaration,
  isConditionalTypeNode,
  isConstructSignatureDeclaration,
  isConstructorDeclaration,
  isConstructorTypeNode,
  isExportDeclaration,
  isFunctionDeclaration,
  isFunctionTypeNode,
  isGetAccessorDeclaration,
  isIdentifier,
  isIndexSignatureDeclaration,
  isInterfaceDeclaration,
  isIntersectionTypeNode,
  isLiteralTypeNode,
  isMappedTypeNode,
  isMethodDeclaration,
  isMethodSignatureDeclaration,
  isModuleDeclaration,
  isNamedExports,
  isNumericLiteral,
  isParenthesizedTypeNode,
  isPrivateIdentifier,
  isPropertyAccessExpression,
  isPropertyDeclaration,
  isPropertySignatureDeclaration,
  isQualifiedName,
  isSetAccessorDeclaration,
  isStringLiteral,
  isTypeAliasDeclaration,
  isTypeLiteralNode,
  isTypeOperatorNode,
  isTypeReferenceNode,
  isUnionTypeNode,
  isVariableStatement,
  type ClassDeclaration,
  type InterfaceDeclaration,
  type Node,
  type SourceFile,
  type Statement,
  type TypeNode,
} from "typescript/unstable/ast";
import { classifySymbol, memberProduct } from "./inventory-classification.ts";

export const GLOBAL_SYMBOL = "(global)";

/** Reviewed target aliases whose object/call/construct shape is intentionally not inventoried. */
export const TYPE_ONLY_TARGET_SYMBOLS: ReadonlySet<string> = new Set();

const OBJECT_UTILITY_TYPES = new Set(["Pick", "Omit", "Partial", "Required", "Readonly", "Record"]);

export interface PendingMember {
  product: string;
  symbol: string;
  member: string;
  kind: string;
  readonly: boolean;
  optional: boolean;
  static: boolean;
  node: Node;
}

export interface DeclarationRow {
  prefix: string;
  statement: Statement;
}

export interface InventoryCoverage {
  named_declarations: number;
  target_declarations: number;
  target_declarations_with_surface: number;
  target_declarations_type_only: number;
}

interface ExpandState {
  product: string;
  symbol: string;
  prefix: string;
  members: PendingMember[];
  seen: Set<Node>;
  visiting: Set<string>;
  index: Map<string, DeclarationRow[]>;
  added: number;
  surface: boolean;
  includeStatic: boolean;
  optionalOverride?: boolean;
  readonlyOverride?: boolean;
  allowedMembers?: ReadonlySet<string>;
}

export function collapse(text: string): string {
  return text.replace(/\s+/g, " ").trim();
}

export function qualify(prefix: string, name: string): string {
  return prefix === "" ? name : `${prefix}${name}`;
}

export function declarationName(node: Node): string | undefined {
  if ("name" in node && node.name !== undefined) {
    const name = node.name as Node;
    if (isIdentifier(name) || isStringLiteral(name)) return name.text;
  }
  return undefined;
}

export function collectStatements(sourceFile: SourceFile): DeclarationRow[] {
  const rows: DeclarationRow[] = [];
  const visit = (statements: readonly Statement[], prefix: string): void => {
    for (const statement of statements) {
      rows.push({ prefix, statement });
      if (isModuleDeclaration(statement) && statement.body !== undefined && "statements" in statement.body) {
        const name = declarationName(statement);
        if (name === undefined) continue;
        visit((statement.body as { statements: readonly Statement[] }).statements, `${qualify(prefix, name)}.`);
      }
    }
  };
  visit(sourceFile.statements, "");
  return rows;
}

export function namedDeclarations(rows: readonly DeclarationRow[]): string[] {
  const names: string[] = [];
  for (const { prefix, statement } of rows) {
    if (isVariableStatement(statement)) {
      for (const declaration of statement.declarationList.declarations) {
        if (isIdentifier(declaration.name)) names.push(qualify(prefix, declaration.name.text));
      }
      continue;
    }
    const name = declarationName(statement);
    if (name !== undefined) names.push(qualify(prefix, name));
  }
  return names;
}

export function exportAliases(statements: readonly Statement[]): Map<string, string> {
  const aliases = new Map<string, string>();
  for (const statement of statements) {
    if (!isExportDeclaration(statement) || statement.exportClause === undefined || !isNamedExports(statement.exportClause)) {
      continue;
    }
    for (const specifier of statement.exportClause.elements) {
      const exported = specifier.name.text;
      const local = specifier.propertyName?.text ?? exported;
      aliases.set(local, exported);
    }
  }
  return aliases;
}

export function buildDeclarationIndex(rows: readonly DeclarationRow[]): Map<string, DeclarationRow[]> {
  const index = new Map<string, DeclarationRow[]>();
  for (const row of rows) {
    if (isVariableStatement(row.statement)) {
      for (const declaration of row.statement.declarationList.declarations) {
        if (!isIdentifier(declaration.name)) continue;
        pushIndex(index, qualify(row.prefix, declaration.name.text), row);
      }
      continue;
    }
    const name = declarationName(row.statement);
    if (name === undefined) continue;
    pushIndex(index, qualify(row.prefix, name), row);
  }
  return index;
}

function pushIndex(index: Map<string, DeclarationRow[]>, name: string, row: DeclarationRow): void {
  const list = index.get(name) ?? [];
  list.push(row);
  index.set(name, list);
}

function propertyNameText(name: Node): string {
  if (isIdentifier(name) || isPrivateIdentifier(name) || isStringLiteral(name) || isNumericLiteral(name)) {
    return name.text;
  }
  return collapse(name.getText());
}

function hasModifier(node: Node, flag: ModifierFlags): boolean {
  const flags = "modifierFlags" in node ? Number(node.modifierFlags) : 0;
  return (flags & flag) !== 0;
}

function isOptionalMember(node: Node): boolean {
  if ("postfixToken" in node && (node as { postfixToken?: Node }).postfixToken?.kind === SyntaxKind.QuestionToken) {
    return true;
  }
  if ("questionToken" in node && (node as { questionToken?: Node }).questionToken !== undefined) {
    return true;
  }
  return false;
}

function isNonPublic(node: Node): boolean {
  return hasModifier(node, ModifierFlags.Private) || hasModifier(node, ModifierFlags.Protected);
}

function entityNameText(node: Node): string | undefined {
  if (isIdentifier(node)) return node.text;
  if (isQualifiedName(node)) {
    const left = entityNameText(node.left);
    return left === undefined ? undefined : `${left}.${node.right.text}`;
  }
  if (isPropertyAccessExpression(node)) {
    const left = entityNameText(node.expression);
    if (left === undefined || !isIdentifier(node.name)) return undefined;
    return `${left}.${node.name.text}`;
  }
  return undefined;
}

function lookup(index: Map<string, DeclarationRow[]>, name: string, prefix: string): DeclarationRow[] {
  if (name.includes(".")) {
    return index.get(name) ?? index.get(qualify(prefix, name)) ?? [];
  }
  let current = prefix;
  while (true) {
    const found = index.get(qualify(current, name));
    if (found !== undefined) return found;
    if (current === "") return [];
    const trimmed = current.endsWith(".") ? current.slice(0, -1) : current;
    const dot = trimmed.lastIndexOf(".");
    current = dot === -1 ? "" : `${trimmed.slice(0, dot + 1)}`;
  }
}

function literalKeys(node: TypeNode): Set<string> | undefined {
  if (isParenthesizedTypeNode(node)) return literalKeys(node.type);
  if (isLiteralTypeNode(node)) {
    if (isStringLiteral(node.literal) || isNumericLiteral(node.literal)) return new Set([node.literal.text]);
    return undefined;
  }
  if (!isUnionTypeNode(node)) return undefined;
  const keys = new Set<string>();
  for (const arm of node.types) {
    const part = literalKeys(arm);
    if (part === undefined) return undefined;
    for (const key of part) keys.add(key);
  }
  return keys;
}

function pushMember(
  state: ExpandState,
  member: string,
  kind: string,
  node: Node,
  flags?: { readonly?: boolean; optional?: boolean; static?: boolean },
): void {
  if (isNonPublic(node) || state.seen.has(node) || (state.allowedMembers !== undefined && !state.allowedMembers.has(member))) return;
  state.seen.add(node);
  state.added += 1;
  state.surface = true;
  const isStatic = flags?.static ?? hasModifier(node, ModifierFlags.Static);
  state.members.push({
    product: memberProduct(state.symbol, member, state.product),
    symbol: state.symbol,
    member,
    kind,
    readonly: flags?.readonly ?? state.readonlyOverride ?? hasModifier(node, ModifierFlags.Readonly),
    optional: flags?.optional ?? state.optionalOverride ?? isOptionalMember(node),
    static: isStatic,
    node,
  });
}

function expandTypeElements(state: ExpandState, nodes: readonly Node[]): void {
  for (const node of nodes) {
    if (!state.includeStatic && hasModifier(node, ModifierFlags.Static)) continue;
    if (isMethodSignatureDeclaration(node) || isMethodDeclaration(node)) {
      pushMember(state, propertyNameText(node.name), "method", node);
    } else if (isPropertySignatureDeclaration(node) || isPropertyDeclaration(node)) {
      pushMember(state, propertyNameText(node.name), "property", node);
    } else if (isConstructorDeclaration(node)) {
      if (state.includeStatic) pushMember(state, "constructor", "constructor", node);
    } else if (isCallSignatureDeclaration(node)) {
      pushMember(state, "()", "call", node);
    } else if (isConstructSignatureDeclaration(node)) {
      pushMember(state, "new", "construct", node);
    } else if (isIndexSignatureDeclaration(node)) {
      pushMember(state, "[]", "index", node);
    } else if (isGetAccessorDeclaration(node)) {
      pushMember(state, propertyNameText(node.name), "get", node);
    } else if (isSetAccessorDeclaration(node)) {
      pushMember(state, propertyNameText(node.name), "set", node);
    }
  }
}

function runWith<T extends keyof ExpandState>(state: ExpandState, patch: Pick<ExpandState, T>, run: () => void): void {
  const previous = {} as Pick<ExpandState, T>;
  for (const key of Object.keys(patch) as T[]) {
    previous[key] = state[key];
    state[key] = patch[key];
  }
  try {
    run();
  } finally {
    for (const key of Object.keys(previous) as T[]) state[key] = previous[key];
  }
}

function expandHeritage(state: ExpandState, statement: InterfaceDeclaration | ClassDeclaration): void {
  if (statement.heritageClauses === undefined) return;
  for (const clause of statement.heritageClauses) {
    if (clause.token !== SyntaxKind.ExtendsKeyword) continue;
    state.surface = true;
    runWith(state, { includeStatic: false }, () => {
      for (const type of clause.types) {
        const name = entityNameText(type.expression);
        if (name !== undefined) expandReference(state, name);
      }
    });
  }
}

function expandObjectUtility(state: ExpandState, name: string, typeArguments: readonly TypeNode[] | undefined, node: Node): void {
  state.surface = true;
  if (name === "Record") {
    pushMember(state, "[]", "index", node);
    return;
  }
  const inner = typeArguments?.[0];
  if (inner === undefined) return;
  if (name === "Partial") {
    runWith(state, { optionalOverride: true }, () => expandType(state, inner));
    return;
  }
  if (name === "Required") {
    runWith(state, { optionalOverride: false }, () => expandType(state, inner));
    return;
  }
  if (name === "Readonly") {
    runWith(state, { readonlyOverride: true }, () => expandType(state, inner));
    return;
  }
  const keyType = typeArguments?.[1];
  const keys = keyType === undefined ? undefined : literalKeys(keyType);
  if (keys === undefined) return;
  const collected: PendingMember[] = [];
  const isolated: ExpandState = {
    ...state,
    members: collected,
    seen: new Set(),
    added: 0,
    surface: false,
  };
  expandType(isolated, inner);
  state.surface = true;
  for (const item of collected) {
    const keep = name === "Pick" ? keys.has(item.member) : !keys.has(item.member);
    if (!keep || state.seen.has(item.node)) continue;
    state.seen.add(item.node);
    state.added += 1;
    state.members.push({
      ...item,
      product: memberProduct(state.symbol, item.member, state.product),
      symbol: state.symbol,
    });
  }
}

function expandResolved(state: ExpandState, name: string, rows: readonly DeclarationRow[]): void {
  if (state.visiting.has(name)) return;
  state.visiting.add(name);
  for (const row of rows) {
    runWith(state, { prefix: row.prefix, includeStatic: false }, () => expandDeclarationShape(state, row.statement));
  }
}

function expandReference(state: ExpandState, name: string): boolean {
  const rows = lookup(state.index, name, state.prefix);
  if (rows.length === 0) return false;
  const resolved = rows[0];
  const qualified = resolved === undefined
    ? name
    : qualify(resolved.prefix, declarationName(resolved.statement) ?? name);
  expandResolved(state, qualified, rows);
  return true;
}

function expandType(state: ExpandState, node: TypeNode, referenceName?: string): void {
  if (isParenthesizedTypeNode(node)) {
    expandType(state, node.type);
    return;
  }
  if (isUnionTypeNode(node) || isIntersectionTypeNode(node)) {
    for (const part of node.types) expandType(state, part);
    return;
  }
  if (isTypeLiteralNode(node)) {
    if (node.members.length) state.surface = true;
    expandTypeElements(state, [...node.members]);
    return;
  }
  if (isFunctionTypeNode(node)) {
    pushMember(state, "()", "call", node);
    return;
  }
  if (isConstructorTypeNode(node)) {
    pushMember(state, "new", "construct", node);
    return;
  }
  if (isMappedTypeNode(node)) {
    pushMember(state, "[]", "index", node);
    return;
  }
  if (isConditionalTypeNode(node)) {
    expandType(state, node.trueType);
    expandType(state, node.falseType);
    return;
  }
  if (isTypeOperatorNode(node) && node.operator === SyntaxKind.ReadonlyKeyword) {
    runWith(state, { readonlyOverride: true }, () => expandType(state, node.type));
    return;
  }
  const name = referenceName ?? (isTypeReferenceNode(node) ? entityNameText(node.typeName) : undefined);
  if (name === undefined) return;
  if (expandReference(state, name)) return;
  if (OBJECT_UTILITY_TYPES.has(name) && isTypeReferenceNode(node)) {
    expandObjectUtility(state, name, node.typeArguments === undefined ? undefined : [...node.typeArguments], node);
  }
}

function expandDeclarationShape(state: ExpandState, statement: Statement): void {
  if (isInterfaceDeclaration(statement)) {
    if (statement.members.length || statement.heritageClauses !== undefined) state.surface = true;
    expandTypeElements(state, [...statement.members]);
    expandHeritage(state, statement);
    return;
  }
  if (isClassDeclaration(statement)) {
    if (statement.members.length || statement.heritageClauses !== undefined) state.surface = true;
    expandTypeElements(state, [...statement.members]);
    expandHeritage(state, statement);
    return;
  }
  if (isTypeAliasDeclaration(statement)) {
    expandType(state, statement.type);
  }
}

export function expandTargetDeclaration(
  members: PendingMember[],
  product: string,
  prefix: string,
  statement: Statement,
  aliases: Map<string, string>,
  index: Map<string, DeclarationRow[]>,
  allowedMembers?: ReadonlySet<string>,
): { added: number; surface: boolean; names: string[] } {
  if (isFunctionDeclaration(statement)) {
    const local = statement.name?.text;
    if (local === undefined) return { added: 0, surface: false, names: [] };
    const exported = aliases.get(local) ?? local;
    const symbol = prefix === "" ? GLOBAL_SYMBOL : prefix.slice(0, -1);
    const state = createState(members, memberProduct(symbol, exported, product), symbol, prefix, index, allowedMembers);
    state.includeStatic = true;
    pushMember(state, exported, "function", statement);
    return { added: state.added, surface: true, names: [qualify(prefix, local)] };
  }
  if (isVariableStatement(statement)) {
    const symbol = prefix === "" ? GLOBAL_SYMBOL : prefix.slice(0, -1);
    const names: string[] = [];
    let added = 0;
    let surface = false;
    for (const declaration of statement.declarationList.declarations) {
      if (!isIdentifier(declaration.name)) continue;
      const qualified = qualify(prefix, declaration.name.text);
      if (classifySymbol(qualified).class !== "target") continue;
      const exported = aliases.get(declaration.name.text) ?? declaration.name.text;
      const state = createState(members, memberProduct(symbol, exported, product), symbol, prefix, index, allowedMembers);
      state.includeStatic = true;
      pushMember(state, exported, "var", declaration);
      added += state.added;
      surface = true;
      names.push(qualified);
    }
    return { added, surface, names };
  }
  const name = declarationName(statement);
  if (name === undefined) return { added: 0, surface: false, names: [] };
  const qualified = qualify(prefix, name);
  const state = createState(members, product, qualified, prefix, index, allowedMembers);
  state.includeStatic = isClassDeclaration(statement) || isInterfaceDeclaration(statement);
  state.visiting.add(qualified);
  expandDeclarationShape(state, statement);
  return { added: state.added, surface: state.surface, names: [qualified] };
}

function createState(
  members: PendingMember[],
  product: string,
  symbol: string,
  prefix: string,
  index: Map<string, DeclarationRow[]>,
  allowedMembers?: ReadonlySet<string>,
): ExpandState {
  return {
    product,
    symbol,
    prefix,
    members,
    seen: new Set(),
    visiting: new Set(),
    index,
    added: 0,
    surface: false,
    includeStatic: false,
    ...(allowedMembers === undefined ? {} : { allowedMembers }),
  };
}

export function emptyCoverage(): InventoryCoverage {
  return {
    named_declarations: 0,
    target_declarations: 0,
    target_declarations_with_surface: 0,
    target_declarations_type_only: 0,
  };
}
