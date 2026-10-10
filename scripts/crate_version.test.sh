#!/usr/bin/env bash
# Pruebas de scripts/crate_version.sh contra repos git de juguete.
#   bash scripts/crate_version.test.sh
set -euo pipefail

script="$(cd "$(dirname "$0")" && pwd)/crate_version.sh"
fails=0

repo() {
    dir=$(mktemp -d)
    git -C "$dir" init -q -b main
    git -C "$dir" config user.email t@t
    git -C "$dir" config user.name t
}
commit() { git -C "$dir" commit -q --allow-empty -m "$1"; }
tag() { git -C "$dir" tag -a "colmena_dag_engine-v$1" -m "$1"; }
expect() {
    local got
    got=$(cd "$dir" && bash "$script" "$1")
    if [[ "$got" == "$2" ]]; then
        echo "ok   $3"
    else
        echo "FAIL $3: esperaba '$2', salió '$got'"
        fails=$((fails + 1))
    fi
}

repo
commit "feat: inicio"
tag 0.31.1
expect beta "" "sin commits nuevos no hay versión"
commit "chore: bump version to 0.3.4"
expect stable "" "un bump del bot no cuenta"
commit "fix(llm): algo"
expect beta 0.31.2-beta.1 "un fix sube la patch"
expect rc 0.31.2-rc.1 "rc cuenta aparte de beta"
commit "feat(http): otra cosa"
expect beta 0.32.0-beta.1 "un feat sube la minor"
tag 0.32.0-beta.1
tag 0.32.0-beta.2
expect beta 0.32.0-beta.3 "N sigue al mayor publicado"
expect stable 0.32.0 "stable sin sufijo"
tag 0.32.0
expect stable "" "stable ya publicado"
commit "refactor!: rompe"
expect beta 0.33.0-beta.1 "en 0.x un cambio que rompe sube la minor"
tag 0.33.0
tag 1.0.0
commit "feat: x"$'\n\n'"BREAKING CHANGE: y"
expect stable 2.0.0 "desde 1.0 BREAKING CHANGE sube la major"

repo
commit "feat: primero"
expect beta 0.1.0-beta.1 "sin tags parte de 0.0.0"

repo
commit "chore: no es del motor"
tag 0.2.0-rc.1
expect rc 0.0.1-rc.1 "los tags con sufijo no son base"

if [[ $fails -gt 0 ]]; then
    echo "$fails fallas"
    exit 1
fi
echo "todo bien"
