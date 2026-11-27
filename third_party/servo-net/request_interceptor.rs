/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use content_security_policy::Destination;
use embedder_traits::{GenericEmbedderProxy, WebResourceRequest, WebResourceResponseMsg};
use log::error;
use net_traits::http_status::HttpStatus;
use net_traits::request::Request;
use net_traits::response::{Response, ResponseBody};
use net_traits::{NetworkError, ResourceFetchTimingContainer};

use crate::async_runtime::spawn_task;
use crate::embedder::NetToEmbedderMsg;
use crate::fetch::methods::{Data, DoneChannel};

#[derive(Clone)]
pub struct RequestInterceptor {
    embedder_proxy: GenericEmbedderProxy<NetToEmbedderMsg>,
}

impl RequestInterceptor {
    pub fn new(embedder_proxy: GenericEmbedderProxy<NetToEmbedderMsg>) -> RequestInterceptor {
        RequestInterceptor { embedder_proxy }
    }

    pub(crate) async fn intercept_request(
        &self,
        request: &mut Request,
        response: &mut Option<Response>,
        timing_info: &ResourceFetchTimingContainer,
        done_chan: &mut DoneChannel,
    ) {
        // Ferrum: las respuestas de audio y vídeo se entregan a medida que llegan
        // en vez de esperar al final. Sin esto, un flujo que no termina (las
        // Media Source Extensions de Ferrum) nunca llegaría al reproductor.
        let streaming = matches!(request.destination, Destination::Audio | Destination::Video);
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let is_for_main_frame = matches!(request.destination, Destination::Document);
        let web_resource_request = WebResourceRequest {
            method: request.method.clone(),
            url: request.url(),
            headers: request.headers.clone(),
            destination: request.destination,
            referrer_url: request.referrer.to_url().cloned(),
            is_for_main_frame,
            is_redirect: request.redirect_count > 0,
        };

        self.embedder_proxy
            .send(NetToEmbedderMsg::WebResourceRequested(
                request.target_webview_id,
                web_resource_request,
                sender,
            ));

        // TODO: use done_chan and run in CoreResourceThreadPool.
        let mut accumulated_body = Vec::new();
        while let Some(message) = receiver.recv().await {
            match message {
                WebResourceResponseMsg::Start(webresource_response) => {
                    let timing = timing_info.inner().clone();
                    let mut response_override =
                        Response::new(webresource_response.url.into(), timing);
                    response_override.headers = webresource_response.headers;
                    response_override.status = HttpStatus::new(
                        webresource_response.status_code,
                        webresource_response.status_message,
                    );
                    if streaming {
                        *response_override.body.lock() = ResponseBody::Receiving(Vec::new());
                        let body = response_override.body.clone();
                        let (data_sender, data_receiver) = tokio::sync::mpsc::unbounded_channel();
                        *done_chan = Some((data_sender.clone(), data_receiver));
                        *response = Some(response_override);
                        spawn_task(async move {
                            while let Some(message) = receiver.recv().await {
                                match message {
                                    WebResourceResponseMsg::SendBodyData(data) => {
                                        let _ = data_sender.send(Data::Payload(data.into()));
                                    },
                                    WebResourceResponseMsg::CancelLoad => {
                                        let _ = data_sender.send(Data::Cancelled);
                                        return;
                                    },
                                    WebResourceResponseMsg::FinishLoad |
                                    WebResourceResponseMsg::DoNotIntercept => break,
                                    WebResourceResponseMsg::Start(_) => {},
                                }
                            }
                            // No se guarda el cuerpo: un flujo de vídeo puede durar horas.
                            *body.lock() = ResponseBody::Done(Vec::new());
                            let _ = data_sender.send(Data::Done);
                        });
                        return;
                    }
                    *response = Some(response_override);
                },
                WebResourceResponseMsg::SendBodyData(data) => {
                    accumulated_body.push(data);
                },
                WebResourceResponseMsg::FinishLoad => {
                    if accumulated_body.is_empty() {
                        break;
                    }
                    let Some(response) = response.as_mut() else {
                        error!("Received unexpected FinishLoad message");
                        break;
                    };
                    *response.body.lock() =
                        ResponseBody::Done(accumulated_body.into_iter().flatten().collect());
                    break;
                },
                WebResourceResponseMsg::CancelLoad => {
                    *response = Some(Response::network_error(NetworkError::LoadCancelled));
                    break;
                },
                WebResourceResponseMsg::DoNotIntercept => break,
            }
        }
    }
}
