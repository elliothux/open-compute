import { atom } from "jotai";

export const detailBreadcrumbAtom = atom<{
  path: string;
  name: string;
} | null>(null);
