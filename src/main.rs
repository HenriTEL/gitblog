use std::error::Error;

use gix_protocol::handshake::Ref;
use gix_protocol::ls_refs;
use gix_url::Url;
use gix_transport::bstr::BStr;
use gix_transport::client::http;
use prodash::progress;

fn main() -> Result<(), Box<dyn Error>> {
    let url = BStr::new("https://github.com/GitoxideLabs/gitoxide");
    let gix_url = Url::from_bytes(url)?;

    let mut transport = http::connect(gix_url, gix_transport::Protocol::default(), false);
    gix_protocol::handshake(
        &mut transport,
        gix_transport::Service::UploadPack,
        &mut |_| Ok(None),
        vec![],
        &mut progress::Discard,
    )?;
    let refs = ls_refs(
        &mut transport,
        &gix_transport::client::Capabilities::default(),
        |_, _, _| Ok(ls_refs::Action::Continue),
        &mut progress::Discard,
        false,
    )?;

    for ref_ in refs {
        match ref_ {
            Ref::Direct { full_ref_name, .. } => println!("{}", full_ref_name),
            _ => (),
        };
    }

    Ok(())
}
