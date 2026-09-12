import "vfile";

// The validator publishes TypeScript source and expects Astro's VFile payload
// to be declared by the consuming project during strict type checking.
declare module "vfile" {
  interface DataMap {
    astro: {
      frontmatter?: Record<
        string,
        boolean | number | object | string | undefined
      >;
    };
  }
}
