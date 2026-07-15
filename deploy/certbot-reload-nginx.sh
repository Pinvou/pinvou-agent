#!/bin/sh
set -eu

nginx -t
systemctl reload nginx
