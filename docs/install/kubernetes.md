# Kubernetes (community-supported sketch)

Folio's first-class deploy target is docker compose for self-hosters.
Kubernetes is supported in the sense that the images are stateless
containers with documented env, health probes, and volume layouts —
but no Helm chart, operator, or maintained manifests ship with the
project. This page sketches the wiring; the rest is the operator's
call.

If you're deploying for a single user / household, compose is much
easier; see [`README.md`](../../README.md#quick-start-operators).

## Topology

Two Deployments + two ClusterIP Services, two PersistentVolumeClaims,
plus managed Postgres + Redis (operator's choice — cloud-managed, or
in-cluster via Bitnami / CloudNative-PG / etc.).

```
Ingress (your choice)
  └── (all paths)  → Service "folio-app" :8080      [the public origin]

Deployment "folio-app"
  └── 1+ replicas; readinessProbe on /readyz; livenessProbe on /healthz
      Env: COMIC_WEB_UPSTREAM_URL=http://folio-web:3000
      Mounts: PVC "folio-data" at /data, PV/PVC for library at /library (ro)

Service "folio-web" (ClusterIP, NOT exposed to Ingress)
  └── reached by folio-app's SSR fallback over the cluster network

Deployment "folio-web"
  └── 1+ replicas; no PVCs; readinessProbe on /

Postgres + Redis: operator-owned (use whatever your cluster has).
```

**Note:** as of M5 of the rust-public-origin rollout, `folio-app` is
the single Ingress target. It reverse-proxies HTML, RSC, and
`/_next/*` requests to `folio-web` internally over the cluster DNS.
You should NOT route any Ingress path to `folio-web` directly — earlier
versions of these docs documented a split (`/api,/auth,...` to app, `/`
to web); that's no longer the supported topology.

## Migrations

For >1 app replica, **do not** rely on `COMIC_AUTO_MIGRATE=true` —
multiple replicas racing through `Migrator::up()` will trip the
`seaql_migrations` advisory lock. Run migrations as a one-shot Job:

```yaml
apiVersion: batch/v1
kind: Job
metadata:
  name: folio-migrate
spec:
  template:
    spec:
      restartPolicy: Never
      containers:
        - name: migrate
          image: ghcr.io/mtbry/folio:v0.1
          command: ["/app/migration", "up"]
          env:
            - name: COMIC_DATABASE_URL
              valueFrom: { secretKeyRef: { name: folio, key: database_url } }
```

Configure your CI/CD to apply this Job before rolling out the app
Deployment.

In the Deployment, set `COMIC_AUTO_MIGRATE=false` so the app doesn't
also try.

## Probes

```yaml
livenessProbe:
  httpGet: { path: /healthz, port: 8080 }
  initialDelaySeconds: 10
  periodSeconds: 30
  failureThreshold: 3
readinessProbe:
  httpGet: { path: /readyz, port: 8080 }
  initialDelaySeconds: 5
  periodSeconds: 5
  failureThreshold: 6   # 30s grace for a flaky DB/Redis blip
startupProbe:
  httpGet: { path: /healthz, port: 8080 }
  periodSeconds: 5
  failureThreshold: 60  # 5 min — covers slow first migration
```

`/healthz` is liveness-only; it returns 200 regardless of dep health.
`/readyz` pings both Postgres and Redis in parallel — if either is
down, the probe returns 503 and the LB will stop sending traffic.

## Sample app Deployment

```yaml
apiVersion: apps/v1
kind: Deployment
metadata: { name: folio-app }
spec:
  replicas: 2
  selector: { matchLabels: { app: folio-app } }
  template:
    metadata: { labels: { app: folio-app } }
    spec:
      securityContext:
        runAsNonRoot: true
        runAsUser: 65532   # distroless nonroot UID
        fsGroup: 65532
      containers:
        - name: app
          image: ghcr.io/mtbry/folio:v0.1
          imagePullPolicy: IfNotPresent
          ports: [{ name: http, containerPort: 8080 }]
          env:
            - name: COMIC_AUTO_MIGRATE
              value: "false"
            - name: COMIC_DATABASE_URL
              valueFrom: { secretKeyRef: { name: folio, key: database_url } }
            - name: COMIC_REDIS_URL
              valueFrom: { secretKeyRef: { name: folio, key: redis_url } }
            - name: COMIC_PUBLIC_URL
              value: https://comics.example.com
            - name: COMIC_TRUSTED_PROXIES
              value: 10.0.0.0/8
            # SSR fallback upstream. Resolved via cluster DNS to the
            # `folio-web` Service. See the topology diagram above.
            - name: COMIC_WEB_UPSTREAM_URL
              value: http://folio-web:3000
          volumeMounts:
            - { name: data,    mountPath: /data }
            - { name: library, mountPath: /library, readOnly: true }
          livenessProbe:   { httpGet: { path: /healthz, port: 8080 }, periodSeconds: 30 }
          readinessProbe:  { httpGet: { path: /readyz,  port: 8080 }, periodSeconds: 5, failureThreshold: 6 }
          startupProbe:    { httpGet: { path: /healthz, port: 8080 }, periodSeconds: 5, failureThreshold: 60 }
          resources:
            requests: { cpu: 200m, memory: 256Mi }
            limits:   { cpu: 2,    memory: 1Gi }
      volumes:
        - name: data
          persistentVolumeClaim: { claimName: folio-data }
        - name: library
          persistentVolumeClaim: { claimName: folio-library }
```

## Storage notes

- `folio-data` PVC: ReadWriteOnce is fine (only one replica at a time
  writes thumbs). 5–20 GB is plenty for most libraries.
- `folio-library` PVC: this is your comic archive. Most clusters mount
  it via NFS / CephFS / a CSI driver pointing at object storage.
  ReadOnlyMany works since the app only reads from it.
- **Secrets** live under `/data/secrets/`. They're auto-generated on
  first boot. For multi-replica deploys this is a problem (each replica
  would generate its own). Two options:
  1. Pre-seed the PVC with secrets via an init container that runs
     before the first app boot (vault-injector pattern).
  2. Run a single-replica `app` Deployment; only the web tier scales out.
     For most homelab Kubernetes installs this is the realistic answer.

## Ingress + cookies

Folio uses `__Host-` cookies which require:
- `Secure` attribute (HTTPS, which your Ingress + cert-manager handles).
- `Path=/` (default).
- No `Domain` attribute.

Some ingress controllers (notably older nginx-ingress) helpfully
rewrite cookies. Don't let them — disable cookie rewriting on the
Ingress object.

```yaml
metadata:
  annotations:
    # nginx-ingress: don't rewrite anything cookie-shaped.
    nginx.ingress.kubernetes.io/proxy-cookie-domain: "off"
    nginx.ingress.kubernetes.io/proxy-cookie-path: "off"
```

## Not covered here

- Helm chart (none ships; community welcome to contribute one).
- HPA + cluster autoscaler tuning.
- Multi-region active-active deploys.
- HA Postgres + Redis topologies.

For these, treat the compose file as the source of truth for env-var
semantics and adapt to your cluster's conventions.
