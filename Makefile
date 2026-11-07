.PHONY: doctor test dev build-macos reinstall-macos release-macos

APP_NAME := Safety Disk Cleaner
APP_PATH := dist/darwin-arm64/$(APP_NAME).app
INSTALL_PATH := /Applications/$(APP_NAME).app

doctor:
	./scripts/doctor.sh

test:
	go test ./...

resources/trash-helper: resources/trash.swift
	xcrun swiftc -O -o $@ $<

dev: doctor resources/trash-helper
	go tool mygo dev

build-macos: doctor resources/trash-helper
	go tool mygo build -platform darwin/arm64 -skip-notarize

reinstall-macos: build-macos
	./scripts/reinstall-macos.sh "$(APP_PATH)" "$(INSTALL_PATH)"

release-macos: doctor resources/trash-helper
	@test -n "$(MACOS_SIGNING_IDENTITY)" || (echo 'Set MACOS_SIGNING_IDENTITY' >&2; exit 1)
	@security find-identity -v -p codesigning | grep -F "$(MACOS_SIGNING_IDENTITY)" >/dev/null || (echo 'Developer ID signing identity not found' >&2; exit 1)
	@xcrun notarytool history --keychain-profile safety-disk-cleaner >/dev/null 2>&1 || (echo 'Notary profile safety-disk-cleaner is unavailable' >&2; exit 1)
	go tool mygo build -platform darwin/arm64 -sign "$(MACOS_SIGNING_IDENTITY)"
