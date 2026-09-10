"""Authenticate HTTP and WebSockets; prevent cross-site WebSocket access."""
from urllib.parse import urlparse
from websockify.auth_plugins import BasicHTTPAuth, AuthenticationError


class PreviewAuth(BasicHTTPAuth):
    def authenticate(self, headers, target_host, target_port):
        super().authenticate(headers, target_host, target_port)
        origin = headers.get('Origin')
        if headers.get('Upgrade', '').lower() == 'websocket':
            if not origin or urlparse(origin).netloc != headers.get('Host'):
                raise AuthenticationError(response_code=403)
