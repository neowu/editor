# Introduction

A code editor forked from zed, only supports MacOS and local development

# Install

```sh
# build locally
./release.sh
./install.sh

# or install a GitHub release (built by pushing an x.y.z tag)
./install.sh x.y.z
```

# Install lsp manually, or it will be run by npx/uvx

```sh
# json, css, html
npm install -g vscode-langservers-extracted
# typescript / javascript: TypeScript 7 (`tsc --lsp`), from the project's node_modules
npm install -D typescript@^7
# lint (js/ts/json/css) and formatting
npm install -D @biomejs/biome

# python
uv tool install ruff
uv tool install ty

# terraform
sudo apt install terraform-ls
```
