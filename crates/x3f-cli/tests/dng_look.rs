use std::process::Command;

#[test]
fn dng_look_is_the_only_supported_flag() {
    for (flag, recognized) in [("-dng-look", true), ("-dcp-look", false)] {
        let output = Command::new(env!("CARGO_BIN_EXE_x3f_extract"))
            .args(["-tiff", flag, "look.dcp", "input.X3F"])
            .output()
            .unwrap();
        assert!(!output.status.success());
        let error = String::from_utf8(output.stderr).unwrap();
        assert_eq!(error.contains("-dng-look requires DNG output"), recognized);
        if !recognized {
            assert!(error.contains("usage:"));
            assert!(error.contains("-dng-look <FILE>"));
            assert!(!error.contains("-dcp-look <FILE>"));
        }
    }
}
