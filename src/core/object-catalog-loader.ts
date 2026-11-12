import { ObjectCatalogController, type ObjectCatalogScope, type ObjectCatalogPort, type ObjectDescriptor, type MetadataSection } from "./object-catalog";

/** 目录分页有界且同revision；错误不能被解释为无对象。 */
export async function loadObjectCatalog(scope: ObjectCatalogScope, port: ObjectCatalogPort): Promise<{ objects: ObjectDescriptor[]; sections: Record<string, MetadataSection>; revision: string }> {
  const controller = new ObjectCatalogController(scope, port);
  try {
    await controller.load();
    let pages = 1;
    while (controller.nextCursor) {
      if (pages >= 64 || controller.objects.length > 4096) throw new Error("OBJECT_CATALOG_LIMIT: 目录超过本次安全分页上限，请缩小对象类别，不能把截断结果当作完整目录");
      await controller.load(true); ++pages;
    }
    if (controller.objects.length > 4096) throw new Error("OBJECT_CATALOG_LIMIT: 对象数超过4096，请缩小范围");
    return { objects: controller.objects, sections: controller.sections, revision: controller.revision };
  } finally { controller.dispose(); }
}
