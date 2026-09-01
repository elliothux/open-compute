import type { NextApiRequest, NextApiResponse } from "next";

export default function handler(_request: NextApiRequest, response: NextApiResponse) {
  response.setHeader("x-p4-router", "pages");
  response.status(200).json({ router: "pages", status: 200 });
}
