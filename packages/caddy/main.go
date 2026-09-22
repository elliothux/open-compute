package main

import (
	caddycmd "github.com/caddyserver/caddy/v2/cmd"
	_ "github.com/caddyserver/caddy/v2/modules/standard"
	_ "open-compute.dev/caddy/provider"
)

func main() {
	caddycmd.Main()
}
