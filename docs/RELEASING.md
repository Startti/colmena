# Releases de colmena_dag_engine

Colmena es privado. El motor se publica **solo** en GitHub Packages (ghcr.io),
también privado. Ya no se publica en PyPI, TestPyPI ni npm: `package.json`
tiene `"private": true` y `pyproject.toml` lleva el clasificador
`Private :: Do Not Upload`, que PyPI rechaza.

## Canales

| Rama      | Versión          | Tag OCI móvil | Quién la usa en ADP              |
|-----------|------------------|---------------|----------------------------------|
| `develop` | `X.Y.Z-beta.N`   | `beta`        | develop                          |
| `staging` | `X.Y.Z-rc.N`     | `rc`          | staging                          |
| `main`    | `X.Y.Z`          | `latest`      | prod (y es obligatorio para prod) |

Cada push a una de esas ramas corre `.github/workflows/publish-crate.yml`, que:

1. calcula la versión con `scripts/crate_version.sh <beta|rc|stable>`;
2. corre fmt, clippy y los tests;
3. empaqueta el crate (`cargo package`) con esa versión;
4. lo sube a `ghcr.io/startti/colmena_dag_engine:<versión>`, junto con
   `docs/node_configurations.json`, y mueve el tag del canal;
5. crea el tag git `colmena_dag_engine-v<versión>` y, en `main`, un GitHub Release.

## Cómo se calcula la versión

La fuente de verdad son los tags `colmena_dag_engine-v*`. El `version` de
`src/libs/colmena/Cargo.toml` no se toca a mano: el CI lo reescribe al empaquetar.

- Base: el último tag estable (sin sufijo).
- Siguiente: por los commits que la base no tiene (Conventional Commits; con
  squash, el título de la PR). `feat` sube la minor; `!:` o `BREAKING CHANGE`
  sube la minor en 0.x y la major desde 1.0; cualquier otro sube la patch.
- beta y rc numeran aparte: `0.32.0-beta.1`, `0.32.0-beta.2`, … y
  `0.32.0-rc.1`, …
- Si no hay commits nuevos desde el último estable, no se publica nada.

**No crees tags `colmena_dag_engine-v*` a mano**: rompen la numeración.

Pruebas del cálculo: `bash scripts/crate_version.test.sh`. Las corre el CI.

## Cómo lo consume ADP

GitHub Packages no tiene registro de Cargo, así que ADP no hace `cargo` contra
un registro: `apps/service/ia/platform/fetch_colmena.sh` baja el artefacto con
[`oras`](https://oras.land) a `apps/service/ia/platform/.colmena/` y los dos
`Cargo.toml` (api y worker) lo usan como dependencia `path`, con la versión
exacta (`version = "=0.32.0-beta.3"`). Cargo valida que la versión del crate
bajado coincida.

Después de publicar, el workflow le avisa a ADP (`workflow_dispatch` de
`colmena-bump.yml` sobre su `develop`) y ADP abre o actualiza una PR hacia su `develop` que sube
el pin, baja el crate y regenera lo que depende de él
(`.github/workflows/colmena-bump.yml` en ADP). Todas van a `develop`, también
los rc y las estables: el pin llega a staging y prod con la promoción normal de
ADP, y su CI frena una beta hacia staging o un pre-release hacia main. El bot
solo propone versiones MAYORES que el pin, así una beta no pisa un rc. Necesita
el secreto `ADP_DISPATCH_TOKEN` en este repo (token con *Actions: write* sobre
`Startti/adp`); sin él, el bump es a mano.

Para que el CI de ADP pueda bajarlo, el paquete tiene que darle acceso al repo
`Startti/adp`: *Package settings → Manage Actions access → Add repository*
(rol Read). Se hace una vez.

## Bajar una versión a mano

    gh auth refresh -s read:packages
    gh auth token | oras login ghcr.io -u <usuario> --password-stdin
    oras pull ghcr.io/startti/colmena_dag_engine:0.32.0-beta.3 -o /tmp/colmena
