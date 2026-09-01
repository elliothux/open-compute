"use server";

import { cookies } from "next/headers";
import { redirect } from "next/navigation";

export async function qualifyAction(form: FormData): Promise<never> {
  const marker = form.get("marker") === "qualified" ? "qualified" : "invalid";
  const jar = await cookies();
  jar.set("p4-action", marker, { httpOnly: true, sameSite: "strict", path: "/" });
  redirect(`/action-result?marker=${marker}`);
}
