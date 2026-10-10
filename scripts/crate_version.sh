#!/usr/bin/env bash
# La versión que publica .github/workflows/publish-crate.yml para HEAD.
#
#   scripts/crate_version.sh <beta|rc|stable>
#
# Imprime la versión (p. ej. 0.32.0-beta.3) o nada si no hay qué publicar.
#
# La fuente de verdad son los tags `colmena_dag_engine-vX.Y.Z[-canal.N]`, no el
# `version` de Cargo.toml (el CI lo reescribe al empaquetar):
#
#   1. Base: el último tag ESTABLE (sin sufijo).
#   2. Siguiente versión, por los commits de HEAD que la base no tiene
#      (Conventional Commits, título del squash): `!:` o BREAKING CHANGE sube
#      la minor mientras estemos en 0.x (la major desde 1.0), `feat` sube la
#      minor, cualquier otro la patch. Los `chore: bump version` no cuentan.
#      Sin commits que cuenten, no hay versión nueva.
#   3. beta y rc: `<siguiente>-<canal>.N`, con N = el mayor ya publicado + 1.
#      stable: `<siguiente>`; si ese tag ya existe, nada.
set -euo pipefail

channel="${1:-}"
case "$channel" in
    beta | rc | stable) ;;
    *)
        echo "uso: $0 <beta|rc|stable>" >&2
        exit 2
        ;;
esac

prefix=colmena_dag_engine-v

last=$(git tag -l "${prefix}*" | sed "s/^${prefix}//" |
    grep -E '^[0-9]+\.[0-9]+\.[0-9]+$' | sort -V | tail -1 || true)

range=(HEAD)
if [[ -n "$last" ]]; then
    range+=("^${prefix}${last}")
else
    last=0.0.0
fi

IFS=. read -r major minor patch <<<"$last"

bump=""
while IFS= read -r -d '' entry; do
    subject=${entry%%$'\n'*}
    subject=${subject#$'\n'}
    [[ -z "$subject" ]] && continue
    [[ "$subject" =~ ^chore:\ bump\ version ]] && continue
    if [[ "$subject" =~ ^[a-z]+(\([^\)]*\))?!: ]] || grep -q '^BREAKING CHANGE' <<<"$entry"; then
        bump=major
    elif [[ "$subject" =~ ^feat(\([^\)]*\))?: ]]; then
        [[ "$bump" == major ]] || bump=minor
    else
        [[ -n "$bump" ]] || bump=patch
    fi
done < <(git log --format='%s%n%b%x00' "${range[@]}")

[[ -n "$bump" ]] || exit 0

# En 0.x un cambio que rompe sube la minor (semver: 0.y.z ya es inestable).
if [[ "$bump" == major && "$major" == 0 ]]; then
    bump=minor
fi
case "$bump" in
    major) next="$((major + 1)).0.0" ;;
    minor) next="${major}.$((minor + 1)).0" ;;
    patch) next="${major}.${minor}.$((patch + 1))" ;;
esac

if [[ "$channel" == stable ]]; then
    git rev-parse -q --verify "refs/tags/${prefix}${next}" >/dev/null && exit 0
    echo "$next"
    exit 0
fi

n=$(git tag -l "${prefix}${next}-${channel}.*" | sed "s/^${prefix}${next}-${channel}\.//" |
    grep -E '^[0-9]+$' | sort -n | tail -1 || true)
echo "${next}-${channel}.$((${n:-0} + 1))"
