import type { NextConfig } from "next";

const config: NextConfig = {
  deploymentId: "oc-p4-vinext-baseline",
  generateBuildId: async () => "oc-p4-vinext-baseline",
  poweredByHeader: false,
};

export default config;
