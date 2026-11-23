package translate

const (
	// Loanwords and acronyms in Japanese can be short runs of Latin letters.
	minLatinCharsForDetection = 3

	// These labels match the free-form target language names in the config.
	langJapanese = "日本語"
	langEnglish  = "英語"
)

// detectLanguage returns an empty string for text too ambiguous to reverse.
func detectLanguage(text string) string {
	hasHan := false
	latinCount := 0
	for _, r := range text {
		switch {
		case isKana(r):
			return langJapanese
		case isHan(r):
			hasHan = true
		case 'A' <= r && r <= 'Z', 'a' <= r && r <= 'z':
			latinCount++
		}
	}
	if !hasHan && latinCount >= minLatinCharsForDetection {
		return langEnglish
	}
	return ""
}

func isKana(r rune) bool {
	return 0x3040 <= r && r <= 0x309f || 0x30a0 <= r && r <= 0x30ff
}

// The reference recognizes these three Han blocks to rule out an English guess.
func isHan(r rune) bool {
	return 0x3400 <= r && r <= 0x4dbf || 0x4e00 <= r && r <= 0x9fff || 0xf900 <= r && r <= 0xfaff
}
