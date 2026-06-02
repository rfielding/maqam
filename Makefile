.PHONY: help claude-setup claude-patch

help:
	@echo "Targets:"
	@echo "  make claude-setup                     # save/load ANTHROPIC_API_KEY locally"
	@echo "  make claude-patch BRANCH=... PROMPT=... # request/apply Claude patch"

claude-setup:
	@./scripts/setup_claude_local.sh

claude-patch:
	@if [ -z "$(BRANCH)" ] || [ -z "$(PROMPT)" ]; then \
		echo "Usage: make claude-patch BRANCH=claude/my-change PROMPT=prompts/fix.md"; \
		exit 1; \
	fi
	@./scripts/claude_patch_loop.sh "$(BRANCH)" "$(PROMPT)"
