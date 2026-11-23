package talk

import (
	"context"
	"fmt"

	"github.com/tik-choco/tc-npc/internal/llm"
)

// ChatTool provides an OpenAI function definition and executes its JSON arguments.
type ChatTool interface {
	Definition() llm.Tool
	Execute(ctx context.Context, args string) (string, error)
}

// ToolRegistry maps function names to tools. Its zero value has no tools.
type ToolRegistry struct {
	tools map[string]ChatTool
}

func (r *ToolRegistry) Register(name string, tool ChatTool) {
	if r.tools == nil {
		r.tools = make(map[string]ChatTool)
	}
	r.tools[name] = tool
}

func (r ToolRegistry) Definitions() []llm.Tool {
	var definitions []llm.Tool
	for _, tool := range r.tools {
		definitions = append(definitions, tool.Definition())
	}
	return definitions
}

// Execute returns errors as tool output so the model can respond to them.
func (r ToolRegistry) Execute(ctx context.Context, name, args string) string {
	tool, ok := r.tools[name]
	if !ok {
		return fmt.Sprintf("Error: tool %s not found", name)
	}
	result, err := tool.Execute(ctx, args)
	if err != nil {
		return fmt.Sprintf("Error: %v", err)
	}
	return result
}
