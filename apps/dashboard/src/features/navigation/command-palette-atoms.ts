import { atom } from "jotai";

export const commandPaletteOpenAtom = atom(false);
export const commandPaletteQueryAtom = atom("");

export const closeCommandPaletteAtom = atom(null, (_get, set) => {
  set(commandPaletteOpenAtom, false);
  set(commandPaletteQueryAtom, "");
});
