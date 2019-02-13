use failure::{err_msg, format_err, Error};
use serde_derive::{Deserialize, Serialize};
use std::fmt;

use crate::{
    AccountId, Addressable, AgentId, Authenticable, Destination, EventSubscription,
    RequestSubscription, ResponseSubscription, SharedGroup, Source,
};

////////////////////////////////////////////////////////////////////////////////

#[derive(Debug)]
pub enum ConnectionMode {
    Agent,
    Bridge,
}

impl fmt::Display for ConnectionMode {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                ConnectionMode::Agent => "agents",
                ConnectionMode::Bridge => "bridge-agents",
            }
        )
    }
}

////////////////////////////////////////////////////////////////////////////////

#[derive(Debug, Deserialize)]
pub struct AgentConfig {
    uri: String,
}

#[derive(Debug)]
pub struct AgentBuilder {
    agent_id: AgentId,
    version: String,
    mode: ConnectionMode,
}

impl AgentBuilder {
    pub fn new(agent_id: AgentId) -> Self {
        Self {
            agent_id,
            version: String::from("v1.mqtt3"),
            mode: ConnectionMode::Agent,
        }
    }

    pub fn version(self, version: &str) -> Self {
        Self {
            agent_id: self.agent_id,
            version: version.to_owned(),
            mode: self.mode,
        }
    }

    pub fn mode(self, mode: ConnectionMode) -> Self {
        Self {
            agent_id: self.agent_id,
            version: self.version,
            mode,
        }
    }

    pub fn start(
        self,
        config: &AgentConfig,
    ) -> Result<(Agent, rumqtt::Receiver<rumqtt::Notification>), Error> {
        let options = Self::mqtt_options(&self.mqtt_client_id(), &config)?;
        let (tx, rx) = rumqtt::MqttClient::start(options)?;

        let agent = Agent::new(self.agent_id, tx);
        Ok((agent, rx))
    }

    fn mqtt_client_id(&self) -> String {
        format!(
            "{version}/{mode}/{agent_id}",
            version = self.version,
            mode = self.mode,
            agent_id = self.agent_id,
        )
    }

    fn mqtt_options(client_id: &str, config: &AgentConfig) -> Result<rumqtt::MqttOptions, Error> {
        let uri = config.uri.parse::<http::Uri>()?;
        let host = uri.host().ok_or_else(|| err_msg("missing MQTT host"))?;
        let port = uri
            .port_part()
            .ok_or_else(|| err_msg("missing MQTT port"))?;

        Ok(rumqtt::MqttOptions::new(client_id, host, port.as_u16())
            .set_keep_alive(30)
            .set_reconnect_opts(rumqtt::ReconnectOptions::AfterFirstSuccess(5)))
    }
}

pub struct Agent {
    id: AgentId,
    tx: rumqtt::MqttClient,
}

impl Agent {
    fn new(id: AgentId, tx: rumqtt::MqttClient) -> Self {
        Self { id, tx }
    }

    pub fn id(&self) -> &AgentId {
        &self.id
    }

    pub fn publish<M>(&mut self, message: &M) -> Result<(), Error>
    where
        M: Publishable,
    {
        let topic = message.destination_topic(&self.id)?;
        let bytes = message.to_bytes()?;

        self.tx
            .publish(topic, QoS::AtLeastOnce, false, bytes)
            .map_err(|_| err_msg("Error publishing an MQTT message"))
    }

    pub fn subscribe<S>(
        &mut self,
        subscription: &S,
        qos: QoS,
        maybe_group: Option<&SharedGroup>,
    ) -> Result<(), Error>
    where
        S: SubscriptionTopic,
    {
        let mut topic = subscription.subscription_topic(&self.id)?;
        if let Some(ref group) = maybe_group {
            topic = format!("$share/{group}/{topic}", group = group, topic = topic);
        };

        self.tx.subscribe(topic, qos)?;
        Ok(())
    }
}

////////////////////////////////////////////////////////////////////////////////

#[derive(Debug, Clone)]
pub struct AuthnProperties {
    agent_id: AgentId,
}

impl Authenticable for AuthnProperties {
    fn account_id(&self) -> &AccountId {
        &self.agent_id.account_id()
    }
}

impl Addressable for AuthnProperties {
    fn agent_id(&self) -> &AgentId {
        &self.agent_id
    }
}

impl From<AgentId> for AuthnProperties {
    fn from(agent_id: AgentId) -> Self {
        Self { agent_id }
    }
}

////////////////////////////////////////////////////////////////////////////////

#[derive(Debug, Deserialize)]
pub struct IncomingEventProperties {
    #[serde(flatten)]
    authn: AuthnProperties,
}

impl Authenticable for IncomingEventProperties {
    fn account_id(&self) -> &AccountId {
        &self.authn.account_id()
    }
}

impl Addressable for IncomingEventProperties {
    fn agent_id(&self) -> &AgentId {
        &self.authn.agent_id()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IncomingRequestProperties {
    method: String,
    correlation_data: String,
    response_topic: String,
    #[serde(flatten)]
    authn: AuthnProperties,
}

impl IncomingRequestProperties {
    pub fn method(&self) -> &str {
        &self.method
    }

    pub fn to_response(
        &self,
        status: &'static OutgoingResponseStatus,
    ) -> OutgoingResponseProperties {
        OutgoingResponseProperties::new(status, &self.correlation_data, Some(&self.response_topic))
    }
}

impl Authenticable for IncomingRequestProperties {
    fn account_id(&self) -> &AccountId {
        &self.authn.account_id()
    }
}

impl Addressable for IncomingRequestProperties {
    fn agent_id(&self) -> &AgentId {
        &self.authn.agent_id()
    }
}

#[derive(Debug, Deserialize)]
pub struct IncomingResponseProperties {
    correlation_data: String,
    #[serde(flatten)]
    authn: AuthnProperties,
}

impl Authenticable for IncomingResponseProperties {
    fn account_id(&self) -> &AccountId {
        &self.authn.account_id()
    }
}

impl Addressable for IncomingResponseProperties {
    fn agent_id(&self) -> &AgentId {
        &self.authn.agent_id()
    }
}

////////////////////////////////////////////////////////////////////////////////

#[derive(Debug)]
pub struct IncomingMessage<T, P>
where
    P: Addressable,
{
    payload: T,
    properties: P,
}

impl<T, P> IncomingMessage<T, P>
where
    P: Addressable,
{
    pub fn new(payload: T, properties: P) -> Self {
        Self {
            payload,
            properties,
        }
    }

    pub fn payload(&self) -> &T {
        &self.payload
    }

    pub fn properties(&self) -> &P {
        &self.properties
    }
}

impl<T> IncomingRequest<T> {
    pub fn to_response<R>(
        &self,
        data: R,
        status: &'static OutgoingResponseStatus,
    ) -> OutgoingResponse<R>
    where
        R: serde::Serialize,
    {
        OutgoingMessage::new(
            data,
            self.properties.to_response(status),
            Destination::Unicast(self.properties().agent_id().clone()),
        )
    }
}

pub type IncomingEvent<T> = IncomingMessage<T, IncomingEventProperties>;
pub type IncomingRequest<T> = IncomingMessage<T, IncomingRequestProperties>;
pub type IncomingResponse<T> = IncomingMessage<T, IncomingResponseProperties>;

////////////////////////////////////////////////////////////////////////////////

#[derive(Debug, Serialize)]
pub struct OutgoingEventProperties {
    label: &'static str,
}

impl OutgoingEventProperties {
    pub fn new(label: &'static str) -> Self {
        Self { label }
    }
}

#[derive(Debug, Serialize)]
pub struct OutgoingRequestProperties {
    method: String,
    correlation_data: String,
    response_topic: String,
    #[serde(flatten)]
    authn: Option<AuthnProperties>,
}

impl OutgoingRequestProperties {
    pub fn new(
        method: String,
        response_topic: String,
        authn: Option<AuthnProperties>,
        correlation_data: String,
    ) -> Self {
        Self {
            method,
            response_topic,
            authn,
            correlation_data,
        }
    }

    pub fn correlation_data(&self) -> &str {
        &self.correlation_data
    }
}

#[derive(Debug, Serialize)]
pub struct OutgoingResponseProperties {
    #[serde(with = "crate::serde::HttpStatusCodeRef")]
    status: &'static OutgoingResponseStatus,
    correlation_data: String,
    #[serde(skip)]
    response_topic: Option<String>,
}

impl OutgoingResponseProperties {
    pub fn new(
        status: &'static OutgoingResponseStatus,
        correlation_data: &str,
        response_topic: Option<&str>,
    ) -> Self {
        Self {
            status,
            correlation_data: correlation_data.to_owned(),
            response_topic: response_topic.map(|val| val.to_owned()),
        }
    }
}

pub type OutgoingResponseStatus = http::StatusCode;

////////////////////////////////////////////////////////////////////////////////

#[derive(Debug)]
pub struct OutgoingMessage<T, P>
where
    T: serde::Serialize,
{
    payload: T,
    properties: P,
    destination: Destination,
}

impl<T, P> OutgoingMessage<T, P>
where
    T: serde::Serialize,
{
    pub fn new(payload: T, properties: P, destination: Destination) -> Self {
        Self {
            payload,
            properties,
            destination,
        }
    }
}

impl<T> OutgoingEvent<T>
where
    T: serde::Serialize,
{
    pub fn broadcast(payload: T, properties: OutgoingEventProperties, to_uri: &str) -> Self {
        OutgoingMessage::new(
            payload,
            properties,
            Destination::Broadcast(to_uri.to_owned()),
        )
    }
}

impl<T> OutgoingRequest<T>
where
    T: serde::Serialize,
{
    pub fn multicast(
        payload: T,
        properties: OutgoingRequestProperties,
        to: &dyn Addressable,
    ) -> Self {
        OutgoingMessage::new(
            payload,
            properties,
            Destination::Multicast(to.account_id().clone()),
        )
    }

    pub fn unicast(
        payload: T,
        properties: OutgoingRequestProperties,
        to: &dyn Addressable,
    ) -> Self {
        OutgoingMessage::new(
            payload,
            properties,
            Destination::Unicast(to.agent_id().clone()),
        )
    }
}

impl<T> OutgoingResponse<T>
where
    T: serde::Serialize,
{
    pub fn unicast(
        payload: T,
        properties: OutgoingResponseProperties,
        to: &dyn Addressable,
    ) -> Self {
        OutgoingMessage::new(
            payload,
            properties,
            Destination::Unicast(to.agent_id().clone()),
        )
    }
}

pub type OutgoingEvent<T> = OutgoingMessage<T, OutgoingEventProperties>;
pub type OutgoingRequest<T> = OutgoingMessage<T, OutgoingRequestProperties>;
pub type OutgoingResponse<T> = OutgoingMessage<T, OutgoingResponseProperties>;

impl<T> compat::IntoEnvelope for OutgoingEvent<T>
where
    T: serde::Serialize,
{
    fn into_envelope(self) -> Result<compat::OutgoingEnvelope, Error> {
        let payload = serde_json::to_string(&self.payload)?;
        let envelope = compat::OutgoingEnvelope::new(
            &payload,
            compat::OutgoingEnvelopeProperties::Event(self.properties),
            self.destination,
        );
        Ok(envelope)
    }
}

impl<T> compat::IntoEnvelope for OutgoingRequest<T>
where
    T: serde::Serialize,
{
    fn into_envelope(self) -> Result<compat::OutgoingEnvelope, Error> {
        let payload = serde_json::to_string(&self.payload)?;
        let envelope = compat::OutgoingEnvelope::new(
            &payload,
            compat::OutgoingEnvelopeProperties::Request(self.properties),
            self.destination,
        );
        Ok(envelope)
    }
}

impl<T> compat::IntoEnvelope for OutgoingResponse<T>
where
    T: serde::Serialize,
{
    fn into_envelope(self) -> Result<compat::OutgoingEnvelope, Error> {
        let payload = serde_json::to_string(&self.payload)?;
        let envelope = compat::OutgoingEnvelope::new(
            &payload,
            compat::OutgoingEnvelopeProperties::Response(self.properties),
            self.destination,
        );
        Ok(envelope)
    }
}

////////////////////////////////////////////////////////////////////////////////

pub trait Publishable {
    fn destination_topic(&self, me: &dyn Addressable) -> Result<String, Error>;
    fn to_bytes(&self) -> Result<String, Error>;
}

////////////////////////////////////////////////////////////////////////////////

pub trait Publish {
    fn publish(&self, tx: &mut Agent) -> Result<(), Error>;
}

impl<T> Publish for T
where
    T: Publishable,
{
    fn publish(&self, tx: &mut Agent) -> Result<(), Error> {
        tx.publish(self)?;
        Ok(())
    }
}

impl<T1, T2> Publish for (T1, T2)
where
    T1: Publishable,
    T2: Publishable,
{
    fn publish(&self, tx: &mut Agent) -> Result<(), Error> {
        tx.publish(&self.0)?;
        tx.publish(&self.1)?;
        Ok(())
    }
}

impl<T> Publish for Vec<T>
where
    T: Publishable,
{
    fn publish(&self, tx: &mut Agent) -> Result<(), Error> {
        for msg in self {
            tx.publish(msg)?;
        }
        Ok(())
    }
}

////////////////////////////////////////////////////////////////////////////////

trait DestinationTopic {
    fn destination_topic(&self, me: &dyn Addressable, dest: &Destination) -> Result<String, Error>;
}

impl DestinationTopic for OutgoingEventProperties {
    fn destination_topic(&self, me: &dyn Addressable, dest: &Destination) -> Result<String, Error> {
        match dest {
            Destination::Broadcast(ref uri) => Ok(format!(
                "apps/{app}/api/v1/{uri}",
                app = me.account_id(),
                uri = uri,
            )),
            _ => Err(format_err!(
                "destination = '{:?}' is incompatible with event message type",
                dest,
            )),
        }
    }
}

impl DestinationTopic for OutgoingRequestProperties {
    fn destination_topic(&self, me: &dyn Addressable, dest: &Destination) -> Result<String, Error> {
        match dest {
            Destination::Unicast(ref agent_id) => Ok(format!(
                "agents/{agent_id}/api/v1/in/{app}",
                agent_id = agent_id,
                app = me.account_id(),
            )),
            Destination::Multicast(ref account_id) => Ok(format!(
                "agents/{agent_id}/api/v1/out/{app}",
                agent_id = me.agent_id(),
                app = account_id,
            )),
            _ => Err(format_err!(
                "destination = '{:?}' is incompatible with request message type",
                dest,
            )),
        }
    }
}

impl DestinationTopic for OutgoingResponseProperties {
    fn destination_topic(&self, me: &dyn Addressable, dest: &Destination) -> Result<String, Error> {
        match &self.response_topic {
            Some(ref val) => Ok(val.to_owned()),
            None => match dest {
                Destination::Unicast(ref agent_id) => Ok(format!(
                    "agents/{agent_id}/api/v1/in/{app}",
                    agent_id = agent_id,
                    app = me.account_id(),
                )),
                _ => Err(format_err!(
                    "destination = '{:?}' is incompatible with response message type",
                    dest,
                )),
            },
        }
    }
}

////////////////////////////////////////////////////////////////////////////////

pub trait SubscriptionTopic {
    fn subscription_topic(&self, agent_id: &dyn Addressable) -> Result<String, Error>;
}

impl<'a> SubscriptionTopic for EventSubscription<'a> {
    fn subscription_topic(&self, _me: &dyn Addressable) -> Result<String, Error> {
        match self.source {
            Source::Broadcast(ref from, ref uri) => Ok(format!(
                "apps/{app}/api/v1/{uri}",
                app = from.account_id(),
                uri = uri,
            )),
            _ => Err(format_err!(
                "source = '{:?}' is incompatible with event subscription",
                self.source,
            )),
        }
    }
}

impl<'a> SubscriptionTopic for RequestSubscription<'a> {
    fn subscription_topic(&self, me: &dyn Addressable) -> Result<String, Error> {
        match self.source {
            Source::Multicast => Ok(format!("agents/+/api/v1/out/{app}", app = me.account_id())),
            Source::Unicast(Some(ref from)) => Ok(format!(
                "agents/{agent_id}/api/v1/in/{app}",
                agent_id = me.agent_id(),
                app = from.account_id(),
            )),
            Source::Unicast(None) => Ok(format!(
                "agents/{agent_id}/api/v1/in/+",
                agent_id = me.agent_id(),
            )),
            _ => Err(format_err!(
                "source = '{:?}' is incompatible with request subscription",
                self.source,
            )),
        }
    }
}

impl<'a> SubscriptionTopic for ResponseSubscription<'a> {
    fn subscription_topic(&self, me: &dyn Addressable) -> Result<String, Error> {
        match self.source {
            Source::Unicast(Some(ref from)) => Ok(format!(
                "agents/{agent_id}/api/v1/in/{app}",
                agent_id = me.agent_id(),
                app = from.account_id(),
            )),
            Source::Unicast(None) => Ok(format!(
                "agents/{agent_id}/api/v1/in/+",
                agent_id = me.agent_id(),
            )),
            _ => Err(format_err!(
                "source = '{:?}' is incompatible with response subscription",
                self.source,
            )),
        }
    }
}

////////////////////////////////////////////////////////////////////////////////

pub mod compat {

    use super::{
        Destination, DestinationTopic, IncomingEvent, IncomingEventProperties, IncomingMessage,
        IncomingRequest, IncomingRequestProperties, IncomingResponse, IncomingResponseProperties,
        OutgoingEventProperties, OutgoingRequestProperties, OutgoingResponseProperties,
        Publishable,
    };
    use crate::Addressable;
    use failure::{err_msg, format_err, Error};
    use serde_derive::{Deserialize, Serialize};

    ////////////////////////////////////////////////////////////////////////////////

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "lowercase")]
    #[serde(tag = "type")]
    pub enum IncomingEnvelopeProperties {
        Event(IncomingEventProperties),
        Request(IncomingRequestProperties),
        Response(IncomingResponseProperties),
    }

    #[derive(Debug, Deserialize)]
    pub struct IncomingEnvelope {
        payload: String,
        properties: IncomingEnvelopeProperties,
    }

    impl IncomingEnvelope {
        pub fn properties(&self) -> &IncomingEnvelopeProperties {
            &self.properties
        }

        pub fn payload<T>(&self) -> Result<T, Error>
        where
            T: serde::de::DeserializeOwned,
        {
            let payload = serde_json::from_str::<T>(&self.payload)?;
            Ok(payload)
        }
    }

    pub fn into_event<T>(envelope: IncomingEnvelope) -> Result<IncomingEvent<T>, Error>
    where
        T: serde::de::DeserializeOwned,
    {
        let payload = envelope.payload::<T>()?;
        match envelope.properties {
            IncomingEnvelopeProperties::Event(props) => Ok(IncomingMessage::new(payload, props)),
            val => Err(format_err!("error converting into event = {:?}", val)),
        }
    }

    pub fn into_request<T>(envelope: IncomingEnvelope) -> Result<IncomingRequest<T>, Error>
    where
        T: serde::de::DeserializeOwned,
    {
        let payload = envelope.payload::<T>()?;
        match envelope.properties {
            IncomingEnvelopeProperties::Request(props) => Ok(IncomingMessage::new(payload, props)),
            _ => Err(err_msg("Error converting into request")),
        }
    }

    pub fn into_response<T>(envelope: IncomingEnvelope) -> Result<IncomingResponse<T>, Error>
    where
        T: serde::de::DeserializeOwned,
    {
        let payload = envelope.payload::<T>()?;
        match envelope.properties {
            IncomingEnvelopeProperties::Response(props) => Ok(IncomingMessage::new(payload, props)),
            _ => Err(err_msg("error converting into response")),
        }
    }

    ////////////////////////////////////////////////////////////////////////////////

    #[derive(Debug, Serialize)]
    #[serde(rename_all = "lowercase")]
    #[serde(tag = "type")]
    pub enum OutgoingEnvelopeProperties {
        Event(OutgoingEventProperties),
        Request(OutgoingRequestProperties),
        Response(OutgoingResponseProperties),
    }

    #[derive(Debug, Serialize)]
    pub struct OutgoingEnvelope {
        payload: String,
        properties: OutgoingEnvelopeProperties,
        #[serde(skip)]
        destination: Destination,
    }

    impl OutgoingEnvelope {
        pub fn new(
            payload: &str,
            properties: OutgoingEnvelopeProperties,
            destination: Destination,
        ) -> Self {
            Self {
                payload: payload.to_owned(),
                properties,
                destination,
            }
        }
    }

    impl DestinationTopic for OutgoingEnvelopeProperties {
        fn destination_topic(
            &self,
            me: &dyn Addressable,
            dest: &Destination,
        ) -> Result<String, Error> {
            match self {
                OutgoingEnvelopeProperties::Event(val) => val.destination_topic(me, dest),
                OutgoingEnvelopeProperties::Request(val) => val.destination_topic(me, dest),
                OutgoingEnvelopeProperties::Response(val) => val.destination_topic(me, dest),
            }
        }
    }

    impl<'a> Publishable for OutgoingEnvelope {
        fn destination_topic(&self, me: &dyn Addressable) -> Result<String, Error> {
            self.properties.destination_topic(me, &self.destination)
        }

        fn to_bytes(&self) -> Result<String, Error> {
            Ok(serde_json::to_string(&self)?)
        }
    }

    ////////////////////////////////////////////////////////////////////////////////

    pub trait IntoEnvelope {
        fn into_envelope(self) -> Result<OutgoingEnvelope, Error>;
    }
}

////////////////////////////////////////////////////////////////////////////////

pub use rumqtt::client::Notification;
pub use rumqtt::QoS;
