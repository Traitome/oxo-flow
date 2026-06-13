//! License header middleware — injects license info into every HTTP response.
//! Uses the existing OXO_FLOW_CONFIG and EMBEDDED_ACADEMIC_LICENSE from lib.rs.

use axum::{
    body::Body,
    http::{HeaderValue, Request, Response},
};
use std::task::{Context, Poll};
use tower::{Layer, Service};

const LICENSE_NOTICE: &str = concat!(
    "oxo-flow v",
    env!("CARGO_PKG_VERSION"),
    " — ",
    "oxo-flow-core, oxo-flow-cli: Apache 2.0. ",
    "oxo-flow-web: Dual license (LICENSE-ACADEMIC / LICENSE-COMMERCIAL). ",
    "Free for academic use. Commercial use requires authorization. ",
    "Contact: Shixiang Wang <wangsx@traitome.com>."
);

pub fn license_header_value() -> HeaderValue {
    HeaderValue::from_static(LICENSE_NOTICE)
}

pub fn license_banner_text() -> String {
    format!(
        "\n  oxo-flow v{} — Academic License\n\
           Free for academic & research use.\n\
           Commercial use requires authorization.\n\
           Contact: Shixiang Wang <wangsx@traitome.com>\n",
        env!("CARGO_PKG_VERSION")
    )
}

#[derive(Clone)]
pub struct LicenseHeaderLayer;

impl<S> Layer<S> for LicenseHeaderLayer {
    type Service = LicenseHeaderMiddleware<S>;

    fn layer(&self, inner: S) -> Self::Service {
        LicenseHeaderMiddleware { inner }
    }
}

#[derive(Clone)]
pub struct LicenseHeaderMiddleware<S> {
    inner: S,
}

impl<S, ReqBody> Service<Request<ReqBody>> for LicenseHeaderMiddleware<S>
where
    S: Service<Request<ReqBody>, Response = Response<Body>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    ReqBody: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = futures::future::BoxFuture<'static, Result<S::Response, S::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);
        Box::pin(async move {
            let mut response = inner.call(req).await?;
            let headers = response.headers_mut();
            if let Ok(val) = HeaderValue::from_str(LICENSE_NOTICE) {
                headers.insert("X-OxoFlow-License", val);
            }
            if let Ok(val) = HeaderValue::from_str(env!("CARGO_PKG_VERSION")) {
                headers.insert("X-OxoFlow-Version", val);
            }
            Ok(response)
        })
    }
}
