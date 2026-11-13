window.onload = function() {
  // Build a system
  let url = window.location.search.match(/url=([^&]+)/);
  if (url && url.length > 1) {
    url = decodeURIComponent(url[1]);
  } else {
    url = window.location.origin;
  }
  let options = {
  "swaggerDoc": {
    "openapi": "3.1.0",
    "info": {
      "title": "Messages",
      "description": "braces in text: { not an object } and a quote: \" {",
      "version": "2"
    },
    "servers": [{ "url": "/v2" }],
    "paths": {
      "/messages": {
        "get": {
          "tags": ["messages"],
          "summary": "List messages",
          "responses": { "200": { "description": "ok }" } }
        }
      }
    }
  },
  "customOptions": {}
};
  url = options.swaggerUrl || url
  let spec1 = options.swaggerDoc
  let ui = SwaggerUIBundle({ spec: spec1, url: url, dom_id: '#swagger-ui' })
  window.ui = ui
}
