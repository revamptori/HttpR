"""HttpR: High-performance HTTPX-compatible HTTP client powered by Rust."""

from ._httpr import (
    URL,
    AsyncClient,
    BasicAuth,
    BearerAuth,
    Client,
    CloseError,
    ConnectError,
    ConnectTimeout,
    Cookies,
    DecodingError,
    DigestAuth,
    Headers,
    HTTPError,
    HTTPStatusError,
    InvalidURL,
    Limits,
    LocalProtocolError,
    NetworkError,
    PoolTimeout,
    ProtocolError,
    ProxyError,
    ReadError,
    ReadTimeout,
    RemoteProtocolError,
    Request,
    RequestError,
    Response,
    StreamError,
    Timeout,
    TimeoutException,
    TooManyRedirects,
    TransportError,
    UnsupportedProtocol,
    WriteError,
    WriteTimeout,
    __version__,
    delete,
    get,
    head,
    options,
    patch,
    post,
    put,
    request,
    stream,
)


class _Codes:
    """Common HTTP status code constants compatible with httpx.codes."""

    CONTINUE = 100
    SWITCHING_PROTOCOLS = 101
    OK = 200
    CREATED = 201
    ACCEPTED = 202
    NO_CONTENT = 204
    MOVED_PERMANENTLY = 301
    FOUND = 302
    SEE_OTHER = 303
    NOT_MODIFIED = 304
    TEMPORARY_REDIRECT = 307
    PERMANENT_REDIRECT = 308
    BAD_REQUEST = 400
    UNAUTHORIZED = 401
    FORBIDDEN = 403
    NOT_FOUND = 404
    METHOD_NOT_ALLOWED = 405
    REQUEST_TIMEOUT = 408
    CONFLICT = 409
    UNPROCESSABLE_ENTITY = 422
    TOO_MANY_REQUESTS = 429
    INTERNAL_SERVER_ERROR = 500
    BAD_GATEWAY = 502
    SERVICE_UNAVAILABLE = 503
    GATEWAY_TIMEOUT = 504


codes = _Codes()

__all__ = [
    "__version__",
    "codes",
    "Client",
    "AsyncClient",
    "Request",
    "Response",
    "Headers",
    "Cookies",
    "URL",
    "Timeout",
    "Limits",
    "BasicAuth",
    "DigestAuth",
    "BearerAuth",
    "request",
    "stream",
    "get",
    "post",
    "put",
    "patch",
    "delete",
    "head",
    "options",
    "HTTPError",
    "RequestError",
    "TransportError",
    "TimeoutException",
    "ConnectTimeout",
    "ReadTimeout",
    "WriteTimeout",
    "PoolTimeout",
    "NetworkError",
    "ConnectError",
    "ReadError",
    "WriteError",
    "CloseError",
    "ProtocolError",
    "LocalProtocolError",
    "RemoteProtocolError",
    "ProxyError",
    "UnsupportedProtocol",
    "DecodingError",
    "TooManyRedirects",
    "HTTPStatusError",
    "InvalidURL",
    "StreamError",
]
