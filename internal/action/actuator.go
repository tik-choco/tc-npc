package action

const (
	minAxisValue float32 = -1
	maxAxisValue float32 = 1
)

// Actuator drives the body's two translation axes, yaw axis, and jump button.
// Implementations are safe for concurrent use and clamp axes to [-1, 1].
// A successful call hands off a command. The caller is responsible for
// releasing a held axis or button, including after cancellation.
type Actuator interface {
	Vertical(value float32) error
	Horizontal(value float32) error
	LookHorizontal(value float32) error
	Jump(pressed bool) error
}
