package config

import "golang.org/x/sys/windows"

func homeDir() string {
	// Use the same profile folder lookup as Rust's dirs crate.
	home, _ := windows.KnownFolderPath(windows.FOLDERID_Profile, 0)
	return home
}
