import sys, json
for line in sys.stdin:
    m = json.loads(line)
    if 'id' not in m: continue
    meth = m['method']
    if meth == 'initialize':
        r = {'protocolVersion': '2025-06-18', 'capabilities': {'tools': {}, 'prompts': {}}, 'serverInfo': {'name': 'weather-demo', 'version': '1.2.0'}}
    elif meth == 'tools/list':
        r = {'tools': [
            {'name': 'get_forecast', 'description': 'Get the weather forecast for a city', 'inputSchema': {'type': 'object', 'properties': {'city': {'type': 'string'}}, 'required': ['city']}},
            {'name': 'get_alerts', 'description': 'Get weather alerts for a US state', 'inputSchema': {'type': 'object', 'properties': {'state': {'type': 'string'}}}}]}
    elif meth == 'tools/call':
        city = m['params']['arguments'].get('city', '?')
        r = {'content': [{'type': 'text', 'text': f'Forecast for {city}: sunny, 22°C, light breeze.'}]}
    elif meth == 'prompts/list':
        r = {'prompts': [{'name': 'trip_plan', 'description': 'Plan a trip around the weather'}]}
    elif meth == 'prompts/get':
        r = {'messages': [{'role': 'user', 'content': {'type': 'text', 'text': 'Plan a 3-day trip based on the forecast.'}}]}
    else:
        r = {}
    print(json.dumps({'jsonrpc': '2.0', 'id': m['id'], 'result': r}), flush=True)
