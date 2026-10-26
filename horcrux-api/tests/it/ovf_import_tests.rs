//! OVF / OVA import against the real binary and the real `qemu-img`: an OVA
//! (tar of descriptor + streamOptimized VMDK, built with the system `tar`) is
//! previewed and imported; the converted qcow2 disk holds the same bytes as
//! the original; hardware is mapped; refusals leave no VM and no staging
//! directory behind.

use crate::support::*;
use serde_json::json;
use std::path::Path;
use std::process::Command;

fn sh(cmd: &str, args: &[&str], dir: &Path) {
    let st = Command::new(cmd)
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap_or_else(|e| panic!("{cmd}: {e}"));
    assert!(st.success(), "{cmd} {args:?}");
}

fn ovf(disk_href: &str, extra_hw: &str, os: &str) -> String {
    format!(
        r#"<?xml version="1.0"?>
<Envelope xmlns="http://schemas.dmtf.org/ovf/envelope/1" xmlns:ovf="http://schemas.dmtf.org/ovf/envelope/1"
  xmlns:rasd="http://schemas.dmtf.org/wbem/wscim/1/cim-schema/2/CIM_ResourceAllocationSettingData" xmlns:vmw="http://www.vmware.com/schema/ovf">
 <References><File ovf:href="{disk_href}" ovf:id="f1"/></References>
 <DiskSection><Info>d</Info><Disk ovf:capacity="1" ovf:capacityAllocationUnits="byte * 2^30" ovf:diskId="d1" ovf:fileRef="f1"/></DiskSection>
 <VirtualSystem ovf:id="qa guest">
  <Info>vm</Info><Name>QA Guest</Name>
  <OperatingSystemSection ovf:id="94"><Info>os</Info><Description>{os}</Description></OperatingSystemSection>
  <VirtualHardwareSection><Info>hw</Info>
   <Item><rasd:ElementName>cpu</rasd:ElementName><rasd:InstanceID>1</rasd:InstanceID><rasd:ResourceType>3</rasd:ResourceType><rasd:VirtualQuantity>2</rasd:VirtualQuantity></Item>
   <Item><rasd:AllocationUnits>byte * 2^20</rasd:AllocationUnits><rasd:ElementName>mem</rasd:ElementName><rasd:InstanceID>2</rasd:InstanceID><rasd:ResourceType>4</rasd:ResourceType><rasd:VirtualQuantity>384</rasd:VirtualQuantity></Item>
   <Item><rasd:ElementName>sata</rasd:ElementName><rasd:InstanceID>3</rasd:InstanceID><rasd:ResourceSubType>AHCI</rasd:ResourceSubType><rasd:ResourceType>20</rasd:ResourceType></Item>
   <Item><rasd:ElementName>disk</rasd:ElementName><rasd:HostResource>ovf:/disk/d1</rasd:HostResource><rasd:InstanceID>4</rasd:InstanceID><rasd:Parent>3</rasd:Parent><rasd:ResourceType>17</rasd:ResourceType></Item>
   <Item><rasd:Connection>VM Network</rasd:Connection><rasd:ElementName>nic</rasd:ElementName><rasd:InstanceID>5</rasd:InstanceID><rasd:ResourceSubType>E1000</rasd:ResourceSubType><rasd:ResourceType>10</rasd:ResourceType></Item>
   {extra_hw}
   <vmw:Config ovf:required="false" vmw:key="firmware" vmw:value="efi"/>
  </VirtualHardwareSection>
 </VirtualSystem>
</Envelope>"#
    )
}

async fn count(a: &Api) -> usize {
    a.get("/vms").await.1.as_array().unwrap().len()
}

#[tokio::test]
async fn ova_is_previewed_converted_and_imported_and_refusals_leave_nothing() {
    let dir = scratch("ovf-import");
    let mut srv = Server::new(&dir);
    srv.start();
    srv.wait_healthy().await;
    let a = srv.api().await;
    let images = srv.data().join("images");
    std::fs::create_dir_all(&images).unwrap();
    let work = dir.join("work");
    std::fs::create_dir_all(&work).unwrap();

    // a 1 GiB raw disk with a recognisable pattern, as streamOptimized VMDK
    let raw = work.join("orig.raw");
    std::fs::write(
        &raw,
        (0..3 << 20).map(|i| (i % 251) as u8).collect::<Vec<u8>>(),
    )
    .unwrap();
    sh(
        "qemu-img",
        &["resize", "-f", "raw", "orig.raw", "1G"],
        &work,
    );
    let want = content_sha(&raw);
    sh(
        "qemu-img",
        &[
            "convert",
            "-f",
            "raw",
            "-O",
            "vmdk",
            "-o",
            "subformat=streamOptimized",
            "orig.raw",
            "guest-disk1.vmdk",
        ],
        &work,
    );
    std::fs::write(work.join("guest.ovf"), ovf("guest-disk1.vmdk", "<Item><rasd:ElementName>cd</rasd:ElementName><rasd:InstanceID>6</rasd:InstanceID><rasd:ResourceType>15</rasd:ResourceType></Item>", "Debian GNU/Linux 12 (64-bit)")).unwrap();
    sh(
        "tar",
        &[
            "-cf",
            &images.join("guest.ova").to_string_lossy(),
            "guest.ovf",
            "guest-disk1.vmdk",
        ],
        &work,
    );
    let ova = images.join("guest.ova").to_string_lossy().to_string();

    // preview: nothing is created
    let (s, v) = a
        .post("/vms/import/ovf/preview", json!({"source": ova}))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["plan"]["name"], "QA Guest");
    assert_eq!(
        (v["plan"]["cpus"].clone(), v["plan"]["memory_mb"].clone()),
        (json!(2), json!(384))
    );
    assert_eq!(v["plan"]["firmware"], "efi");
    assert_eq!(v["plan"]["disks"][0]["bus"], "sata");
    assert_eq!(v["plan"]["nics"][0]["model"], "e1000");
    assert!(v["plan"]["warnings"].to_string().contains("CD-ROM"), "{v}");
    assert_eq!(v["id"], "ovf-qa-guest");
    let (_, vms) = a.get("/vms").await;
    assert!(
        vms.as_array()
            .unwrap()
            .iter()
            .all(|x| x["id"] != "ovf-qa-guest"),
        "preview created a VM"
    );

    // import
    let (s, v) = a.post("/vms/import/ovf", json!({"source": ova})).await;
    assert_eq!(s, 201, "{v}");
    let vm = &v["vm"];
    assert_eq!(vm["id"], "ovf-qa-guest");
    assert_eq!(
        (
            vm["cpus"].clone(),
            vm["memory"].clone(),
            vm["status"].clone()
        ),
        (json!(2), json!(384), json!("stopped"))
    );
    // UEFI guest: OVMF when the host has it, else BIOS plus a warning that says so
    let (_, opts) = a.get("/vms/host-options").await;
    if opts["ovmf"] == true {
        assert_eq!(vm["hardware"]["firmware"], "ovmf");
    } else {
        assert_ne!(vm["hardware"]["firmware"], "ovmf");
        assert!(
            v["warnings"].to_string().contains("OVMF is not installed"),
            "{v}"
        );
    }
    assert_eq!(vm["disks"][0]["disk_type"], "sata");
    assert_eq!(vm["disks"][0]["size_gb"], 1);
    let disk = vm["disks"][0]["path"].as_str().unwrap().to_string();
    let info = run("qemu-img", &["info", "--output=json", &disk]);
    assert!(
        info.contains("\"format\": \"qcow2\"") && info.contains("1073741824"),
        "{info}"
    );
    assert_eq!(
        content_sha(Path::new(&disk)),
        want,
        "disk content survived the conversion"
    );
    // the staging directory is gone
    let leftovers: Vec<_> = std::fs::read_dir(srv.data())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("ovf-import-"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
    // importing again gets a fresh id; an explicit id/name is honoured
    let (s, v) = a
        .post(
            "/vms/import/ovf",
            json!({"source": ova, "id": "second", "name": "Second"}),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(
        (v["vm"]["id"].clone(), v["vm"]["name"].clone()),
        (json!("second"), json!("Second"))
    );

    // refusals: nothing is left behind
    let before = count(&a).await;
    // outside the allowed directories
    let (s, _) = a
        .post("/vms/import/ovf", json!({"source": "/etc/passwd"}))
        .await;
    assert!(s == 403 || s == 400, "{s}");
    // wrong kind of file
    std::fs::write(images.join("notes.txt"), "x").unwrap();
    let (s, _) = a
        .post(
            "/vms/import/ovf",
            json!({"source": images.join("notes.txt").to_string_lossy()}),
        )
        .await;
    assert_eq!(s, 400);
    // a path-like disk name
    std::fs::write(work.join("evil.ovf"), ovf("../../etc/passwd", "", "Linux")).unwrap();
    std::fs::copy(work.join("evil.ovf"), images.join("evil.ovf")).unwrap();
    let (s, v) = a
        .post(
            "/vms/import/ovf",
            json!({"source": images.join("evil.ovf").to_string_lossy()}),
        )
        .await;
    assert_eq!(s, 400, "{v}");
    assert!(v.to_string().contains("plain file name"), "{v}");
    // a DTD
    std::fs::write(images.join("dtd.ovf"), "<?xml version=\"1.0\"?><!DOCTYPE e [<!ENTITY x SYSTEM \"file:///etc/passwd\">]><Envelope/>").unwrap();
    let (s, v) = a
        .post(
            "/vms/import/ovf",
            json!({"source": images.join("dtd.ovf").to_string_lossy()}),
        )
        .await;
    assert_eq!(s, 400, "{v}");
    assert!(v.to_string().contains("DOCTYPE"), "{v}");
    // the descriptor names a disk the archive does not hold
    std::fs::write(work.join("missing.ovf"), ovf("nothere.vmdk", "", "Linux")).unwrap();
    sh(
        "tar",
        &[
            "-cf",
            &images.join("missing.ova").to_string_lossy(),
            "missing.ovf",
        ],
        &work,
    );
    let (s, v) = a
        .post(
            "/vms/import/ovf",
            json!({"source": images.join("missing.ova").to_string_lossy()}),
        )
        .await;
    assert_eq!(s, 400, "{v}");
    assert!(v.to_string().contains("nothere.vmdk"), "{v}");
    // a VMDK whose extent is another file is refused by the qemu-img checks
    std::fs::write(work.join("secret"), vec![0x5au8; 1 << 20]).unwrap();
    std::fs::write(
        work.join("ext.vmdk"),
        "# Disk DescriptorFile\nversion=1\nCID=fffffffe\nparentCID=ffffffff\ncreateType=\"monolithicFlat\"\n\nRW 2048 FLAT \"secret\" 0\n",
    )
    .unwrap();
    std::fs::write(work.join("ext.ovf"), ovf("ext.vmdk", "", "Linux")).unwrap();
    sh(
        "tar",
        &[
            "-cf",
            &images.join("ext.ova").to_string_lossy(),
            "ext.ovf",
            "ext.vmdk",
            "secret",
        ],
        &work,
    );
    let (s, v) = a
        .post(
            "/vms/import/ovf",
            json!({"source": images.join("ext.ova").to_string_lossy()}),
        )
        .await;
    assert_eq!(s, 400, "{v}");
    assert_eq!(count(&a).await, before, "failed imports must not leave VMs");
    let leftovers: Vec<_> = std::fs::read_dir(srv.data())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("ovf-import-"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");

    // .ovf with the disk next to it
    std::fs::copy(
        work.join("guest-disk1.vmdk"),
        images.join("guest-disk1.vmdk"),
    )
    .unwrap();
    std::fs::copy(work.join("guest.ovf"), images.join("guest.ovf")).unwrap();
    let (s, v) = a
        .post(
            "/vms/import/ovf",
            json!({"source": images.join("guest.ovf").to_string_lossy(), "id": "plain-ovf"}),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let disk = v["vm"]["disks"][0]["path"].as_str().unwrap().to_string();
    assert_eq!(content_sha(Path::new(&disk)), want);
}
