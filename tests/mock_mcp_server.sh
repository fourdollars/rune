#!/bin/bash
# Mock MCP server for testing — speaks JSON-RPC over stdio
# Responds to: initialize, tools/list, tools/call

while IFS= read -r line; do
    method=$(echo "$line" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('method',''))" 2>/dev/null)
    id=$(echo "$line" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('id',''))" 2>/dev/null)

    case "$method" in
        "initialize")
            echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"protocolVersion\":\"2024-11-05\",\"capabilities\":{},\"serverInfo\":{\"name\":\"mock-mcp\",\"version\":\"1.0\"}}}"
            ;;
        "notifications/initialized")
            # No response needed for notifications
            ;;
        "tools/list")
            echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"tools\":[{\"name\":\"mock_echo\",\"description\":\"Echo back the input\",\"inputSchema\":{\"type\":\"object\",\"properties\":{\"text\":{\"type\":\"string\"}}}}]}}"
            ;;
        "tools/call")
            text=$(echo "$line" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('params',{}).get('arguments',{}).get('text','hello'))" 2>/dev/null)
            echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"echo: $text\"}]}}"
            ;;
        *)
            echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"error\":{\"code\":-32601,\"message\":\"Method not found: $method\"}}"
            ;;
    esac
done
