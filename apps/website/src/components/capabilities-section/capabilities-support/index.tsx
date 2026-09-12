import styles from "./styles.module.css";

const projectRows = [
  [
    { name: "React", icon: "/assets/tech-icons/react.webp" },
    { name: "Vite", icon: "/assets/tech-icons/vite.webp" },
    { name: "Astro", icon: "/assets/tech-icons/astro.webp" },
    {
      name: "React Router",
      icon: "/assets/tech-icons/reactrouter.webp",
    },
    { name: "Next.js", icon: "/assets/tech-icons/nextjs.webp" },
    { name: "Vue.js", icon: "/assets/tech-icons/vue.webp" },
    { name: "SvelteKit", icon: "/assets/tech-icons/svelte.webp" },
    { name: "TanStack Start", icon: "/assets/tech-icons/tanstack.webp" },
    { name: "Expo", icon: "/assets/tech-icons/expo.webp" },
  ],
  [
    { name: "Hono", icon: "/assets/tech-icons/hono.webp" },
    { name: "FastAPI", icon: "/assets/tech-icons/fastapi.webp" },
    { name: "Flask", icon: "/assets/tech-icons/flask.webp" },
    { name: "Python", icon: "/assets/tech-icons/python.webp" },
    { name: "TypeScript", icon: "/assets/tech-icons/typescript.webp" },
    { name: "Rust", icon: "/assets/tech-icons/rust.webp" },
    { name: "WebAssembly", icon: "/assets/tech-icons/webassembly.webp" },
    { name: "Vercel AI SDK", icon: "/assets/tech-icons/vercel.webp" },
    { name: "LangChain", icon: "/assets/tech-icons/langchain.webp" },
  ],
] as const;

type ProjectRow = (typeof projectRows)[number];

function LogoSequence({
  projects,
  duplicate = false,
}: {
  projects: ProjectRow;
  duplicate?: boolean;
}) {
  return (
    <div
      className="capabilities__support-logo-sequence"
      aria-hidden={duplicate ? "true" : undefined}
    >
      {projects.map((project) => (
        <span className="capabilities__support-logo-tile" key={project.name}>
          <img src={project.icon} alt="" />
          <strong>{project.name}</strong>
        </span>
      ))}
    </div>
  );
}

function LogoMarquee() {
  return (
    <div className="capabilities__support-marquees" aria-hidden="true">
      {projectRows.map((projects, index) => {
        const direction = index === 0 ? "forward" : "reverse";
        return (
          <div
            className={`capabilities__support-marquee capabilities__support-marquee--${direction}`}
            key={direction}
          >
            <div className="capabilities__support-logo-track">
              <LogoSequence projects={projects} />
              <LogoSequence projects={projects} duplicate />
            </div>
          </div>
        );
      })}
    </div>
  );
}

export function CapabilitiesSupport() {
  return (
    <div className={`capabilities__support ${styles.module}`}>
      <article className="capabilities__support-card capabilities__support-card--logos">
        <div className="capabilities__support-copy">
          <h3 className="capabilities__support-title">
            Works with your ecosystem
          </h3>
        </div>
        <LogoMarquee />
      </article>
    </div>
  );
}
