use failure::*;

use crate::tools;
use crate::api::schema::*;
use crate::api::router::*;
use serde_json::{json, Value};


fn get_subscription(_param: Value, _info: &ApiMethod) -> Result<Value, Error> {

    let url = "https://www.proxmox.com/en/proxmox-backup-server/pricing";
    Ok(json!({
        "status": "NotFound",
	"message": "There is no subscription key",
	"serverid": tools::get_hardware_address()?,
	"url":  url,
     }))
}

pub fn router() -> Router {

    let route = Router::new()
        .get(ApiMethod::new(
            get_subscription,
            ObjectSchema::new("Read subscription info.")));

    route
}
