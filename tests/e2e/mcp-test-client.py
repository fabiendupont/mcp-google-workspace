#!/usr/bin/env python3
"""Lightweight MCP test client — no navra dependency.

Connects to the MCP server via streamable HTTP, sends prompts to
Ollama or Anthropic models, and loops tool calls until done.

Usage:
    python3 mcp-test-client.py --mcp http://127.0.0.1:3100/mcp \
        --model gemma4:26b --prompt "List my upcoming calendar events"

    python3 mcp-test-client.py --mcp http://127.0.0.1:3100/mcp \
        --model gemma4:26b --prompt-file prompts/test-gmail-workflow.md

    # With Anthropic API (requires ANTHROPIC_API_KEY)
    python3 mcp-test-client.py --mcp http://127.0.0.1:3100/mcp \
        --provider anthropic --model claude-sonnet-4-5-20250514 \
        --prompt "List my Drive files"
"""

import argparse
import json
import sys
import time

import requests


def parse_sse(response_text):
    """Parse SSE response, return list of data payloads."""
    results = []
    for line in response_text.split("\n"):
        if line.startswith("data: "):
            data = line[6:]
            if data.strip():
                try:
                    results.append(json.loads(data))
                except json.JSONDecodeError:
                    pass
    return results


class McpClient:
    def __init__(self, base_url):
        self.base_url = base_url
        self.session_id = None
        self.request_id = 0
        self.headers = {
            "Content-Type": "application/json",
            "Accept": "application/json, text/event-stream",
        }

    def _next_id(self):
        self.request_id += 1
        return self.request_id

    def _post(self, method, params=None):
        body = {
            "jsonrpc": "2.0",
            "id": self._next_id(),
            "method": method,
        }
        if params:
            body["params"] = params

        headers = dict(self.headers)
        if self.session_id:
            headers["Mcp-Session-Id"] = self.session_id

        resp = requests.post(self.base_url, json=body, headers=headers, timeout=120)
        if "mcp-session-id" in resp.headers:
            self.session_id = resp.headers["mcp-session-id"]

        payloads = parse_sse(resp.text)
        for p in payloads:
            if "result" in p:
                return p["result"]
            if "error" in p:
                return {"error": p["error"]}
        return None

    def initialize(self):
        return self._post("initialize", {
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": {"name": "mcp-test-client", "version": "1.0"},
        })

    def list_tools(self):
        result = self._post("tools/list")
        if result and "tools" in result:
            return result["tools"]
        return []

    def call_tool(self, name, arguments):
        result = self._post("tools/call", {"name": name, "arguments": arguments})
        return result


class OllamaProvider:
    def __init__(self, model, base_url="http://localhost:11434"):
        self.model = model
        self.base_url = base_url

    def chat(self, messages, tools):
        body = {
            "model": self.model,
            "messages": messages,
            "tools": self._convert_tools(tools),
            "stream": False,
        }
        resp = requests.post(f"{self.base_url}/api/chat", json=body, timeout=300)
        resp.raise_for_status()
        return resp.json()

    def _convert_tools(self, mcp_tools):
        ollama_tools = []
        for t in mcp_tools:
            schema = t.get("inputSchema", {})
            ollama_tools.append({
                "type": "function",
                "function": {
                    "name": t["name"],
                    "description": t.get("description", ""),
                    "parameters": schema,
                },
            })
        return ollama_tools


class AnthropicProvider:
    def __init__(self, model, api_key):
        self.model = model
        self.api_key = api_key
        self.base_url = "https://api.anthropic.com/v1"

    def chat(self, messages, tools):
        system = None
        api_messages = []
        for m in messages:
            if m["role"] == "system":
                system = m["content"]
            else:
                api_messages.append(m)

        body = {
            "model": self.model,
            "max_tokens": 4096,
            "messages": api_messages,
            "tools": self._convert_tools(tools),
        }
        if system:
            body["system"] = system

        resp = requests.post(
            f"{self.base_url}/messages",
            json=body,
            headers={
                "x-api-key": self.api_key,
                "anthropic-version": "2023-06-01",
                "content-type": "application/json",
            },
            timeout=300,
        )
        resp.raise_for_status()
        return self._convert_response(resp.json())

    def _convert_tools(self, mcp_tools):
        return [
            {
                "name": t["name"],
                "description": t.get("description", ""),
                "input_schema": t.get("inputSchema", {}),
            }
            for t in mcp_tools
        ]

    def _convert_response(self, resp):
        message = {"role": "assistant"}
        tool_calls = []
        text_parts = []

        for block in resp.get("content", []):
            if block["type"] == "tool_use":
                tool_calls.append({
                    "function": {
                        "name": block["name"],
                        "arguments": block["input"],
                    },
                })
            elif block["type"] == "text":
                text_parts.append(block["text"])

        if tool_calls:
            message["tool_calls"] = tool_calls
        message["content"] = "\n".join(text_parts) if text_parts else ""

        return {"message": message, "done_reason": resp.get("stop_reason", "stop")}


def run_agent_loop(mcp, provider, prompt, tools, max_iterations=50):
    messages = [
        {"role": "system", "content": "You are a helpful assistant with access to Google Workspace tools. Use the tools to accomplish the user's request. When done, summarize what you did."},
        {"role": "user", "content": prompt},
    ]

    tool_log = []
    start_time = time.time()

    for iteration in range(1, max_iterations + 1):
        response = provider.chat(messages, tools)
        msg = response.get("message", {})

        tool_calls = msg.get("tool_calls", [])
        if not tool_calls:
            elapsed = time.time() - start_time
            print(f"\n--- Done in {iteration} iterations, {elapsed:.1f}s ---")
            if msg.get("content"):
                print(f"\nAssistant: {msg['content'][:500]}")
            return tool_log

        messages.append(msg)

        for tc in tool_calls:
            fn = tc.get("function", {})
            name = fn.get("name", "")
            args = fn.get("arguments", {})
            if isinstance(args, str):
                try:
                    args = json.loads(args)
                except json.JSONDecodeError:
                    args = {}

            print(f"  [{iteration}] {name}({json.dumps(args)[:80]})")

            result = mcp.call_tool(name, args)
            tool_log.append({"tool": name, "args": args, "result_ok": result and not result.get("isError")})

            content = ""
            if result:
                if "content" in result:
                    for c in result["content"]:
                        if c.get("type") == "text":
                            content += c.get("text", "")
                elif "error" in result:
                    content = f"Error: {result['error']}"

            messages.append({
                "role": "tool",
                "content": content[:8000],
            })

    print(f"\n--- Max iterations ({max_iterations}) reached ---")
    return tool_log


def main():
    parser = argparse.ArgumentParser(description="MCP test client")
    parser.add_argument("--mcp", default="http://127.0.0.1:3100/mcp", help="MCP server URL")
    parser.add_argument("--model", default="gemma4:26b", help="Model name")
    parser.add_argument("--provider", default="ollama", choices=["ollama", "anthropic"], help="LLM provider")
    parser.add_argument("--prompt", help="Prompt text")
    parser.add_argument("--prompt-file", help="Read prompt from file")
    parser.add_argument("--max-iterations", type=int, default=50, help="Max tool call iterations")
    parser.add_argument("--ollama-url", default="http://localhost:11434", help="Ollama API URL")
    parser.add_argument("--log", help="Write tool call log to file (JSONL)")
    args = parser.parse_args()

    if args.prompt_file:
        with open(args.prompt_file) as f:
            prompt = f.read().strip()
    elif args.prompt:
        prompt = args.prompt
    else:
        print("Error: --prompt or --prompt-file required", file=sys.stderr)
        sys.exit(1)

    if args.provider == "anthropic":
        import os
        api_key = os.environ.get("ANTHROPIC_API_KEY")
        if not api_key:
            print("Error: ANTHROPIC_API_KEY env var required for anthropic provider", file=sys.stderr)
            sys.exit(1)
        provider = AnthropicProvider(args.model, api_key)
    else:
        provider = OllamaProvider(args.model, args.ollama_url)

    print(f"MCP: {args.mcp}")
    print(f"Model: {args.provider}/{args.model}")
    print(f"Prompt: {prompt[:80]}...")

    mcp = McpClient(args.mcp)
    print("Initializing MCP session...")
    mcp.initialize()

    tools = mcp.list_tools()
    print(f"{len(tools)} tools available")

    tool_log = run_agent_loop(mcp, provider, prompt, tools, args.max_iterations)

    if args.log:
        with open(args.log, "w") as f:
            for entry in tool_log:
                f.write(json.dumps(entry) + "\n")
        print(f"Tool log: {args.log}")

    errors = sum(1 for t in tool_log if not t.get("result_ok"))
    print(f"\nTotal tool calls: {len(tool_log)}, errors: {errors}")


if __name__ == "__main__":
    main()
