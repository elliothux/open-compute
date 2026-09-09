use super::*;

pub(super) fn run() {
    let private = TcpListener::bind("127.0.0.1:0").expect("bind private listener");
    let public = TcpListener::bind("127.0.0.1:0").expect("bind public listener");
    let private_port = private.local_addr().unwrap().port();
    let public_port = public.local_addr().unwrap().port();
    let private_server = respond_once(private, 404);
    let public_server = respond_once(public, 200);

    assert_eq!(
        health_port_from(&[private_port, public_port]),
        Some(public_port)
    );
    private_server.join().expect("private response thread");
    public_server.join().expect("public response thread");
}
