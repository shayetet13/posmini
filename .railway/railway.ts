import { defineRailway, github, project, service, volume } from "railway/iac";

// Two services built from this repository's root (so both Dockerfiles can see the whole repo):
//   backend  - Rust API, SQLite on a volume, no public domain (reached over the private network)
//   frontend - Caddy: static pages + reverse proxy to the backend; the only public service
//
// Secrets (ADMIN_PASSWORD, ...) are NOT managed here (variables: { managed: false } below);
// set them in Railway or with `railway variable set`. See README.md for the list.
//
//   railway config plan    preview what would change
//   railway config apply   apply it
export default defineRailway(() => {
  // checkSuites = "Wait for CI": Railway deploys a commit only after its GitHub Actions pass.
  const posmini = github("shayetet13/posmini", { branch: "main", checkSuites: true });

  const backendVolume = volume("backend-volume", {
    alerts: { usage: { "100": {}, "80": {}, "95": {} } },
    allowOnlineResize: true,
    region: "asia-southeast1-eqsg3a",
    sizeMB: 5000,
  });

  const frontend = service("frontend", {
    source: posmini,
    build: {
      builder: "DOCKERFILE",
      dockerfilePath: "frontend/Dockerfile",
      watchPatterns: ["frontend/**"],
    },
    deploy: {
      healthcheckPath: "/health",
      healthcheckTimeout: 120,
      restartPolicyType: "ON_FAILURE",
      restartPolicyMaxRetries: 5,
    },
    replicas: { "asia-southeast1-eqsg3a": 1 },
  });

  const backend = service("backend", {
    source: posmini,
    build: {
      builder: "DOCKERFILE",
      dockerfilePath: "backend/Dockerfile",
      // The backend image embeds frontend/public for the all-in-one mode, but the cloud build does
      // not need a redeploy for page-only changes.
      watchPatterns: ["backend/**", "Cargo.toml", "Cargo.lock"],
    },
    deploy: {
      healthcheckPath: "/health",
      healthcheckTimeout: 120,
      restartPolicyType: "ON_FAILURE",
      restartPolicyMaxRetries: 5,
    },
    // SQLite allows one writer process: keep a single replica.
    replicas: { "asia-southeast1-eqsg3a": 1 },
    volumeMounts: { "/data": backendVolume },
  });

  return project("posmini", {
    variables: { managed: false },
    resources: [frontend, backend, backendVolume],
  });
});
